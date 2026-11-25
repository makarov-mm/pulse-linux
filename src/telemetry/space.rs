// SPDX-License-Identifier: GPL-3.0-or-later

//! Measures the disk space under a folder, the way `du -x` does.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

/// Files listed individually in each folder. The rest are summed into one entry,
/// which bounds memory on folders with very many files.
const LARGEST_FILES: usize = 20;
/// `st_blocks` counts 512-byte units on every filesystem.
const BLOCK_BYTES: u64 = 512;

/// A folder, a file, or the sum of a folder's smaller files.
///
/// The scanner publishes a folder's children once and then keeps adding to the sizes
/// of the folders above it, so a reader sees sizes grow while a scan runs.
pub struct SpaceNode {
    pub name: String,
    pub is_folder: bool,
    /// Greater than zero for the entry that stands for this many smaller files.
    pub grouped: usize,
    size: AtomicU64,
    unread: AtomicBool,
    children: Mutex<Vec<Arc<SpaceNode>>>,
}

impl SpaceNode {
    fn new(name: String, is_folder: bool, size: u64, grouped: usize) -> Arc<Self> {
        Arc::new(Self {
            name,
            is_folder,
            grouped,
            size: AtomicU64::new(size),
            unread: AtomicBool::new(false),
            children: Mutex::default(),
        })
    }

    /// Bytes allocated on disk.
    pub fn size(&self) -> u64 {
        self.size.load(Ordering::Relaxed)
    }

    /// The folder, or something in it, could not be read, so its size is a lower bound.
    pub fn unread(&self) -> bool {
        self.unread.load(Ordering::Relaxed)
    }

    pub fn children(&self) -> Vec<Arc<SpaceNode>> {
        self.children.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// A scan running on its own thread. Dropping it cancels the scan.
pub struct Scan {
    pub root: Arc<SpaceNode>,
    cancel: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}

impl Scan {
    pub fn start(path: PathBuf) -> std::io::Result<Self> {
        let root = SpaceNode::new(path.to_string_lossy().into_owned(), true, 0, 0);
        let cancel = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        // Folders are walked recursively, so deep trees need more than the default stack.
        thread::Builder::new()
            .name("space-scan".into())
            .stack_size(32 << 20)
            .spawn({
                let (root, cancel, finished) = (root.clone(), cancel.clone(), finished.clone());
                move || {
                    let mut walk = Walk {
                        device: None,
                        cancel: &cancel,
                        links: HashSet::new(),
                        above: Vec::new(),
                    };
                    walk.folder(&root, &path);
                    finished.store(true, Ordering::Release);
                }
            })?;
        Ok(Self { root, cancel, finished })
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

impl Drop for Scan {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Walk<'a> {
    /// The filesystem of the scan's root. Other filesystems mounted below it are not entered.
    device: Option<u64>,
    cancel: &'a AtomicBool,
    /// Files with several hard links are counted the first time one of the links is met.
    links: HashSet<(u64, u64)>,
    /// The folders from the root down to the one being read.
    above: Vec<Arc<SpaceNode>>,
}

impl Walk<'_> {
    fn folder(&mut self, node: &Arc<SpaceNode>, path: &Path) {
        if self.cancel.load(Ordering::Relaxed) {
            return;
        }
        self.above.push(node.clone());
        let mut folders = Vec::new();
        let mut largest = BinaryHeap::new();
        let (mut own_bytes, mut file_bytes, mut files) = (0, 0, 0);

        // Symbolic links are never followed: an entry's metadata describes the link itself.
        match (fs::symlink_metadata(path), fs::read_dir(path)) {
            (Ok(own), Ok(entries)) => {
                own_bytes = own.blocks() * BLOCK_BYTES;
                let device = *self.device.get_or_insert(own.dev());
                for entry in entries {
                    let Some((entry, metadata)) = entry.ok().and_then(|entry| {
                        let metadata = entry.metadata().ok()?;
                        Some((entry, metadata))
                    }) else {
                        self.mark_unread();
                        continue;
                    };
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if metadata.is_dir() {
                        if metadata.dev() == device {
                            folders.push(SpaceNode::new(name, true, 0, 0));
                        }
                        continue;
                    }
                    if metadata.nlink() > 1 && !self.links.insert((metadata.dev(), metadata.ino())) {
                        continue;
                    }
                    let size = metadata.blocks() * BLOCK_BYTES;
                    file_bytes += size;
                    files += 1;
                    largest.push(Reverse((size, name)));
                    if largest.len() > LARGEST_FILES {
                        largest.pop();
                    }
                }
            }
            _ => self.mark_unread(),
        }

        let mut children = folders.clone();
        let mut listed_bytes = 0;
        for Reverse((size, name)) in largest {
            listed_bytes += size;
            children.push(SpaceNode::new(name, false, size, 0));
        }
        if files > LARGEST_FILES {
            children.push(SpaceNode::new(
                String::new(),
                false,
                file_bytes - listed_bytes,
                files - LARGEST_FILES,
            ));
        }
        *node.children.lock().unwrap_or_else(PoisonError::into_inner) = children;
        // The folder's own blocks count towards it, as they do in `du`.
        for folder in &self.above {
            folder.size.fetch_add(own_bytes + file_bytes, Ordering::Relaxed);
        }

        for folder in folders {
            let path = path.join(&folder.name);
            self.folder(&folder, &path);
        }
        self.above.pop();
    }

    fn mark_unread(&self) {
        for folder in &self.above {
            folder.unread.store(true, Ordering::Relaxed);
        }
    }
}
