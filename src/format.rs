// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::CStr;
use std::sync::OnceLock;

use crate::i18n::{mark, tr, trf};

const UNITS: [&str; 5] = [mark("B"), mark("KiB"), mark("MiB"), mark("GiB"), mark("TiB")];
pub const NO_DATA: &str = "—";

/// A number with `digits` decimals and the decimal separator of the user's locale.
pub fn number(value: f64, digits: usize) -> String {
    let text = format!("{value:.digits$}");
    match decimal_point() {
        "." => text,
        point => text.replace('.', point),
    }
}

/// The locale's decimal separator, read once. GTK has selected the locale by the time anything is formatted.
fn decimal_point() -> &'static str {
    static POINT: OnceLock<String> = OnceLock::new();
    POINT.get_or_init(|| {
        // SAFETY: `localeconv` returns a pointer to static storage whose `decimal_point` is a
        // NUL-terminated string. It is read at once, on the thread that owns the locale.
        let point = unsafe { CStr::from_ptr((*libc::localeconv()).decimal_point) };
        match point.to_str() {
            Ok(point) if !point.is_empty() => point.to_owned(),
            _ => ".".to_owned(),
        }
    })
}

/// A byte count as a number and its binary unit, for example `("1.5", "GiB")`.
pub fn size(bytes: f64) -> (String, String) {
    let mut value = bytes.max(0.0);
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let digits = if unit == 0 || value >= 100.0 { 0 } else { 1 };
    (number(value, digits), tr(UNITS[unit]))
}

pub fn bytes(bytes: f64) -> String {
    let (number, unit) = size(bytes);
    format!("{number} {unit}")
}

pub fn rate(bytes_per_second: Option<f64>) -> String {
    bytes_per_second.map_or_else(|| NO_DATA.to_owned(), |value| trf("{0}/s", &[&bytes(value)]))
}

pub fn percent(value: Option<f64>) -> String {
    value.map_or_else(|| NO_DATA.to_owned(), |value| format!("{} %", number(value, 1)))
}
