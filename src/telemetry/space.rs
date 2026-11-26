// SPDX-License-Identifier: GPL-3.0-or-later

//! Measures the disk space under a folder, the way `du -x` does.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
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
                    walk.folder(&root, open_root(&path));
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

fn open_directory(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
}

/// procfs keeps lookups relative to this open directory even if an ancestor is renamed.
fn directory_path(directory: &fs::File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
}

fn open_root(path: &Path) -> io::Result<fs::File> {
    let mut directory = open_directory(if path.is_absolute() {
        Path::new("/")
    } else {
        Path::new(".")
    })?;
    for component in path.components() {
        if matches!(component, Component::Normal(_) | Component::ParentDir) {
            directory = open_directory(&directory_path(&directory).join(component.as_os_str()))?;
        }
    }
    Ok(directory)
}

impl Walk<'_> {
    fn folder(&mut self, node: &Arc<SpaceNode>, directory: io::Result<fs::File>) {
        if self.cancel.load(Ordering::Relaxed) {
            return;
        }
        self.above.push(node.clone());
        if directory.and_then(|dir| self.read_folder(node, &dir)).is_err() {
            self.mark_unread();
        }
        self.above.pop();
    }

    fn read_folder(&mut self, node: &SpaceNode, directory: &fs::File) -> io::Result<()> {
        let own = directory.metadata()?;
        let device = *self.device.get_or_insert(own.dev());
        if own.dev() != device {
            return Ok(());
        }
        let mut folders = Vec::new();
        let mut largest = BinaryHeap::new();
        let (mut file_bytes, mut files) = (0, 0);
        for entry in fs::read_dir(directory_path(directory))? {
            if self.cancel.load(Ordering::Relaxed) {
                break;
            }
            let Ok((entry, metadata)) = entry.and_then(|entry| {
                let metadata = entry.metadata()?;
                Ok((entry, metadata))
            }) else {
                self.mark_unread();
                continue;
            };
            if metadata.dev() != device {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if metadata.is_dir() {
                folders.push((entry.path(), SpaceNode::new(name, true, 0, 0)));
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

        let mut children: Vec<_> = folders.iter().map(|(_, folder)| folder.clone()).collect();
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
            folder
                .size
                .fetch_add(own.blocks() * BLOCK_BYTES + file_bytes, Ordering::Relaxed);
        }

        for (path, folder) in folders {
            if self.cancel.load(Ordering::Relaxed) {
                break;
            }
            self.folder(&folder, open_directory(&path));
        }
        Ok(())
    }

    fn mark_unread(&self) {
        for folder in &self.above {
            folder.unread.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    #[test]
    fn scans_raw_names_and_keeps_symlinks_out_of_the_walk() {
        let temp = std::env::temp_dir().join(format!(
            "pulse-space-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&temp).unwrap();
        let raw = temp.join(OsStr::from_bytes(b"folder-\xff"));
        fs::create_dir(&raw).unwrap();
        fs::write(raw.join("payload"), [1_u8; 4096]).unwrap();
        symlink(&raw, temp.join("shortcut")).unwrap();

        let cancel = AtomicBool::new(false);
        let mut walk = Walk {
            device: None,
            cancel: &cancel,
            links: HashSet::new(),
            above: Vec::new(),
        };
        let root = SpaceNode::new("root".into(), true, 0, 0);
        walk.folder(&root, open_root(&temp));
        let children = root.children();
        assert!(!root.unread());
        let real = children.iter().find(|node| node.is_folder).unwrap();
        assert_eq!(real.children()[0].name, "payload");
        assert!(children.iter().any(|node| node.name == "shortcut" && !node.is_folder));
        assert!(open_root(&temp.join("shortcut")).is_err());
        assert!(open_root(&temp.join("shortcut/.")).is_err());

        // Enter the enumeration with cancellation already requested. The folder still
        // contains files, so an entry-only check in folder() would fail this assertion.
        cancel.store(true, Ordering::Relaxed);
        let partial = SpaceNode::new("partial".into(), true, 0, 0);
        walk.read_folder(&partial, &open_root(&temp).unwrap()).unwrap();
        assert!(partial.children().is_empty());
        fs::remove_dir_all(temp).unwrap();
    }
}
