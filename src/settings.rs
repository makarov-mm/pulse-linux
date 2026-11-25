// SPDX-License-Identifier: GPL-3.0-or-later

//! What the application remembers between runs.

use gtk::gio;

use crate::APP_ID;

/// The stored settings, or `None` when the schema is not installed, as under a plain `cargo run`.
/// Asking GSettings for a schema it does not know ends the process, hence the lookup first.
pub fn load() -> Option<gio::Settings> {
    gio::SettingsSchemaSource::default()?.lookup(APP_ID, true)?;
    Some(gio::Settings::new(APP_ID))
}
