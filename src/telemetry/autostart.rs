// SPDX-License-Identifier: GPL-3.0-or-later

//! Programs started at login through the XDG autostart directories.
//!
//! An entry is a desktop file. A file in the user's directory overrides the file of the same
//! name in the system directories, and `Hidden=true` there switches the entry off.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gtk::glib;

const GROUP: &str = "Desktop Entry";
/// GNOME's own switch, which some entries carry instead of `Hidden`.
const GNOME_ENABLED: &str = "X-GNOME-Autostart-enabled";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The desktop file's name, which identifies the entry across directories.
    pub id: String,
    pub name: String,
    pub command: String,
    pub enabled: bool,
}

/// Where autostart entries live: the user's directory first, then the system ones in order of precedence.
#[derive(Clone, Debug)]
pub struct Locations {
    pub user: PathBuf,
    pub system: Vec<PathBuf>,
}

impl Locations {
    pub fn current() -> Self {
        Self {
            user: glib::user_config_dir().join("autostart"),
            system: glib::system_config_dirs()
                .into_iter()
                .map(|dir| dir.join("autostart"))
                .collect(),
        }
    }

    /// The file that defines an entry: the first one found in order of precedence.
    fn find(&self, id: &str) -> Option<PathBuf> {
        std::iter::once(&self.user)
            .chain(&self.system)
            .map(|dir| dir.join(id))
            .find(|path| path.is_file())
    }
}

/// Every entry once, sorted by name.
pub fn read(locations: &Locations) -> Vec<Entry> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for directory in std::iter::once(&locations.user).chain(&locations.system) {
        let Ok(files) = fs::read_dir(directory) else { continue };
        for file in files.flatten() {
            let Some(id) = file.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            // A directory read earlier takes precedence, even when its file cannot be parsed.
            if !id.ends_with(".desktop") || !seen.insert(id.clone()) {
                continue;
            }
            if let Some(entry) = load(&file.path()).and_then(|keys| describe(id, &keys)) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by_key(|entry| entry.name.to_lowercase());
    entries
}

fn load(path: &Path) -> Option<glib::KeyFile> {
    let keys = glib::KeyFile::new();
    let flags = glib::KeyFileFlags::KEEP_COMMENTS | glib::KeyFileFlags::KEEP_TRANSLATIONS;
    keys.load_from_file(path, flags).ok().map(|()| keys)
}

fn describe(id: String, keys: &glib::KeyFile) -> Option<Entry> {
    if keys.string(GROUP, "Type").is_ok_and(|kind| kind != "Application") {
        return None;
    }
    let hidden = keys.boolean(GROUP, "Hidden").unwrap_or(false);
    let switched_off = keys.boolean(GROUP, GNOME_ENABLED).is_ok_and(|enabled| !enabled);
    let name = keys.locale_string(GROUP, "Name", None).map(String::from);
    Some(Entry {
        name: name.unwrap_or_else(|_| id.trim_end_matches(".desktop").to_owned()),
        command: keys.string(GROUP, "Exec").map(String::from).unwrap_or_default(),
        enabled: !hidden && !switched_off,
        id,
    })
}

/// Switches an entry on or off for this user.
///
/// The change is written to the user's directory. A system entry is copied there first,
/// because the copy is what overrides it. System files are never modified.
pub fn set_enabled(locations: &Locations, id: &str, enabled: bool) -> io::Result<()> {
    // The name comes from a directory listing, but it must still stay inside the autostart directory.
    if id.contains('/') || !id.ends_with(".desktop") {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let source = locations.find(id).ok_or(io::ErrorKind::NotFound)?;
    let keys = load(&source).ok_or(io::ErrorKind::InvalidData)?;
    keys.set_boolean(GROUP, "Hidden", !enabled);
    if keys.has_key(GROUP, GNOME_ENABLED).unwrap_or(false) {
        keys.set_boolean(GROUP, GNOME_ENABLED, enabled);
    }
    fs::create_dir_all(&locations.user)?;
    keys.save_to_file(locations.user.join(id)).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A user and a system autostart directory under a fresh temporary folder.
    fn fixture(name: &str) -> Locations {
        let root = std::env::temp_dir().join(format!("pulse-autostart-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let locations = Locations {
            user: root.join("user"),
            system: vec![root.join("system")],
        };
        fs::create_dir_all(&locations.system[0]).expect("the fixture folder is writable");
        locations
    }

    fn write(directory: &Path, id: &str, body: &str) {
        fs::create_dir_all(directory).expect("the fixture folder is writable");
        fs::write(directory.join(id), format!("[Desktop Entry]\nType=Application\n{body}"))
            .expect("the entry is written");
    }

    #[test]
    fn a_user_file_overrides_the_system_entry() {
        let locations = fixture("override");
        write(&locations.system[0], "sync.desktop", "Name=Sync\nExec=sync --daemon\n");
        write(
            &locations.system[0],
            "tray.desktop",
            "Name=Tray\nExec=tray\nX-GNOME-Autostart-enabled=false\n",
        );
        write(
            &locations.user,
            "sync.desktop",
            "Name=Sync\nExec=sync --daemon\nHidden=true\n",
        );
        fs::write(locations.system[0].join("notes.txt"), "not an entry").expect("the file is written");

        let entries = read(&locations);
        let summary: Vec<_> = entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.enabled))
            .collect();
        assert_eq!(summary, [("Sync", false), ("Tray", false)]);
        assert_eq!(entries[0].command, "sync --daemon");
    }

    #[test]
    fn switching_writes_only_to_the_user_directory() {
        let locations = fixture("switch");
        let body = "Name=Backup\nName[de]=Sicherung\nExec=backup\n";
        write(&locations.system[0], "backup.desktop", body);
        let system_file = locations.system[0].join("backup.desktop");
        let original = fs::read_to_string(&system_file).expect("the system entry is readable");

        set_enabled(&locations, "backup.desktop", false).expect("the entry is switched off");
        assert!(!read(&locations)[0].enabled);
        assert_eq!(
            fs::read_to_string(&system_file).expect("the system entry is readable"),
            original
        );
        let copy = fs::read_to_string(locations.user.join("backup.desktop")).expect("the user copy exists");
        assert!(copy.contains("Name[de]=Sicherung") && copy.contains("Hidden=true"));

        set_enabled(&locations, "backup.desktop", true).expect("the entry is switched on");
        assert!(read(&locations)[0].enabled);
        assert_eq!(
            set_enabled(&locations, "../backup.desktop", true).map_err(|e| e.kind()),
            Err(io::ErrorKind::InvalidInput)
        );
        assert_eq!(
            set_enabled(&locations, "missing.desktop", true).map_err(|e| e.kind()),
            Err(io::ErrorKind::NotFound)
        );
    }
}
