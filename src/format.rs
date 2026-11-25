// SPDX-License-Identifier: GPL-3.0-or-later

use crate::i18n::{mark, tr, trf};

const UNITS: [&str; 5] = [mark("B"), mark("KiB"), mark("MiB"), mark("GiB"), mark("TiB")];
pub const NO_DATA: &str = "—";

/// A byte count as a number and its binary unit, for example `("1.5", "GiB")`.
pub fn size(bytes: f64) -> (String, String) {
    let mut value = bytes.max(0.0);
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let digits = if unit == 0 || value >= 100.0 { 0 } else { 1 };
    (format!("{value:.digits$}"), tr(UNITS[unit]))
}

pub fn bytes(bytes: f64) -> String {
    let (number, unit) = size(bytes);
    format!("{number} {unit}")
}

pub fn rate(bytes_per_second: Option<f64>) -> String {
    bytes_per_second.map_or_else(|| NO_DATA.to_owned(), |value| trf("{0}/s", &[&bytes(value)]))
}

pub fn percent(value: Option<f64>) -> String {
    value.map_or_else(|| NO_DATA.to_owned(), |value| format!("{value:.1} %"))
}
