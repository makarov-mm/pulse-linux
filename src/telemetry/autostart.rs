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

    /// A user override containing only Hidden masks the whole system entry. Use the
    /// underlying launch definition for display and when writing an enabled override.
    fn definition(&self, id: &str, source: &Path) -> io::Result<(glib::KeyFile, bool)> {
        let keys = load(source).ok_or(io::ErrorKind::InvalidData)?;
        if launchable(&keys) {
            return Ok((keys, false));
        }
        if source == self.user.join(id) && keys.has_key(GROUP, "Hidden").unwrap_or(false) {
            let system = self.system.iter().map(|dir| dir.join(id)).find(|path| path.is_file());
            if let Some(keys) = system.as_deref().and_then(load).filter(launchable) {
                return Ok((keys, true));
            }
        }
        Err(io::ErrorKind::InvalidData.into())
    }
}

/// Entries applicable to this session, sorted by name.
pub fn read(locations: &Locations) -> Vec<Entry> {
    read_for_desktops(locations, &std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default())
}

fn read_for_desktops(locations: &Locations, desktops: &str) -> Vec<Entry> {
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
            if let Ok((keys, masked)) = locations.definition(&id, &file.path())
                && let Some(entry) = describe(id, &keys, masked, desktops)
            {
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

fn launchable(keys: &glib::KeyFile) -> bool {
    keys.string(GROUP, "Exec")
        .is_ok_and(|command| !command.trim().is_empty())
        || keys.boolean(GROUP, "DBusActivatable").unwrap_or(false)
}

fn describe(id: String, keys: &glib::KeyFile, masked: bool, desktops: &str) -> Option<Entry> {
    let matches_desktop = |key| {
        keys.string_list(GROUP, key).ok().map(|list| {
            desktops
                .split(':')
                .filter(|name| !name.is_empty())
                .any(|name| list.iter().any(|item| item.as_str() == name))
        })
    };
    if keys.string(GROUP, "Type").is_ok_and(|kind| kind != "Application")
        || matches_desktop("OnlyShowIn") == Some(false)
        || matches_desktop("NotShowIn") == Some(true)
    {
        return None;
    }
    if let Ok(program) = keys.string(GROUP, "TryExec")
        && !program.is_empty()
        && ((!Path::new(program.as_str()).is_absolute() && program.contains('/'))
            || glib::find_program_in_path(program.as_str()).is_none())
    {
        return None;
    }
    let hidden = keys.boolean(GROUP, "Hidden").unwrap_or(false);
    let switched_off = keys.boolean(GROUP, GNOME_ENABLED).is_ok_and(|enabled| !enabled);
    let name = keys.locale_string(GROUP, "Name", None).map(String::from);
    Some(Entry {
        name: name.unwrap_or_else(|_| id.trim_end_matches(".desktop").to_owned()),
        command: keys.string(GROUP, "Exec").map(String::from).unwrap_or_default(),
        enabled: !masked && !hidden && !switched_off,
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
    let (keys, _) = locations.definition(id, &source)?;
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

    #[test]
    fn a_minimal_mask_can_restore_the_highest_priority_system_definition() {
        let mut locations = fixture("mask");
        locations.system.push(locations.user.with_file_name("other-system"));
        let id = "agent.desktop";
        write(
            &locations.system[0],
            id,
            "Name=First\nExec=first --agent\nOnlyShowIn=GNOME;\n",
        );
        write(&locations.system[1], id, "Name=Second\nExec=second\n");
        fs::create_dir_all(&locations.user).unwrap();
        let mask = locations.user.join(id);
        // Also repair a mask left with Hidden=false by an earlier version.
        for hidden in [true, false] {
            fs::write(&mask, format!("[Desktop Entry]\nHidden={hidden}\n")).unwrap();
            assert!(read_for_desktops(&locations, "KDE").is_empty());
            let entries = read_for_desktops(&locations, "GNOME");
            assert_eq!(entries.len(), 1);
            assert!(!entries[0].enabled);
            set_enabled(&locations, id, true).unwrap();
            let restored = read_for_desktops(&locations, "GNOME");
            assert!(restored[0].enabled);
            assert_eq!(restored[0].command, "first --agent");
            assert!(read_for_desktops(&locations, "KDE").is_empty());
        }
    }

    #[test]
    fn startup_respects_desktop_lists_and_try_exec() {
        let locations = fixture("session");
        let system = &locations.system[0];
        write(system, "only.desktop", "Name=Only\nExec=app\nOnlyShowIn=GNOME;MATE;\n");
        write(system, "except.desktop", "Name=Except\nExec=app\nNotShowIn=KDE;\n");
        write(
            system,
            "missing.desktop",
            "Exec=app\nTryExec=pulse-test-no-such-program-8dae39e0\n",
        );
        write(system, "present.desktop", "Name=Present\nExec=app\nTryExec=sh\n");
        let names = |desktops| {
            read_for_desktops(&locations, desktops)
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names("Unity:GNOME"), ["Except", "Only", "Present"]);
        assert_eq!(names("KDE"), ["Present"]);
        assert_eq!(names(""), ["Except", "Present"]);
    }
}
