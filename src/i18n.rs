// SPDX-License-Identifier: GPL-3.0-or-later

//! Translation through gettext. Source strings are English and double as message IDs.
//!
//! Arguments are numbered (`{0}`, `{1}`) so that a translation can reorder them.

use std::fmt::Display;
use std::path::PathBuf;

use gettextrs::{bind_textdomain_codeset, bindtextdomain, gettext, ngettext, textdomain};
use gtk::glib;

pub const DOMAIN: &str = "pulse";

/// Selects the message catalogue. The locale itself is set by GTK when the application starts,
/// which is before the first string is looked up.
///
/// The catalogue directory is fixed by the build (`LOCALEDIR`, set by Meson).
/// `PULSE_LOCALEDIR` overrides it for running a build that is not installed.
pub fn init() {
    let directory = std::env::var_os("PULSE_LOCALEDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(option_env!("LOCALEDIR").unwrap_or("/usr/share/locale")));
    let bound = bindtextdomain(DOMAIN, directory)
        .and_then(|_| bind_textdomain_codeset(DOMAIN, "UTF-8").map(|_| ()))
        .and_then(|_| textdomain(DOMAIN).map(|_| ()));
    if let Err(error) = bound {
        glib::g_warning!(DOMAIN, "Translations are unavailable: {error}");
    }
}

/// Marks a string for translation where it is defined and translated later with [`tr`].
pub const fn mark(text: &'static str) -> &'static str {
    text
}

pub fn tr(text: &str) -> String {
    gettext(text)
}

/// Translates `text` and puts `arguments` in place of `{0}`, `{1}` and so on.
pub fn trf(text: &str, arguments: &[&dyn Display]) -> String {
    fill(gettext(text), arguments)
}

/// Translates with the plural form for `count`, which is also argument `{0}`.
pub fn trn(singular: &str, plural: &str, count: usize) -> String {
    let form = ngettext(singular, plural, u32::try_from(count).unwrap_or(u32::MAX));
    fill(form, &[&count])
}

fn fill(mut text: String, arguments: &[&dyn Display]) -> String {
    for (index, argument) in arguments.iter().enumerate() {
        text = text.replace(&format!("{{{index}}}"), &argument.to_string());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_fill_their_numbered_places() {
        assert_eq!(fill("{1} of {0}".to_owned(), &[&"total", &3]), "3 of total");
        assert_eq!(fill("{0} and {0}".to_owned(), &[&"x"]), "x and x");
        assert_eq!(fill("no places".to_owned(), &[&1]), "no places");
    }
}
