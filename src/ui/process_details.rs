// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::*;
use gtk::{gio, glib};

use super::process_list::ProcessObject;
use crate::format::NO_DATA;
use crate::i18n::{tr, trf};
use crate::telemetry::{self, ProcessDetails};

/// Reads the details of `process` off the main loop and shows them in a dialog over `window`.
/// A process that has exited in the meantime is reported through `toasts` instead.
pub fn present(window: &adw::ApplicationWindow, toasts: &adw::ToastOverlay, process: &ProcessObject) {
    let (key, name, user) = (process.key(), process.name(), process.user());
    let (window, toasts) = (window.downgrade(), toasts.clone());
    glib::spawn_future_local(async move {
        let details = gio::spawn_blocking(move || telemetry::details(key))
            .await
            .ok()
            .flatten();
        let Some(window) = window.upgrade() else { return };
        let Some(details) = details else {
            toasts.add_toast(adw::Toast::new(&trf("“{0}” has exited.", &[&name])));
            return;
        };

        let group = adw::PreferencesGroup::new();
        for (title, value) in rows(key.pid, &user, &details) {
            // Values come from the process and are shown as plain text, not markup.
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle(value.as_deref().unwrap_or(NO_DATA))
                .subtitle_selectable(true)
                .use_markup(false)
                .css_classes(["property"])
                .build();
            group.add(&row);
        }
        let page = adw::PreferencesPage::new();
        page.add(&group);
        let view = adw::ToolbarView::builder().content(&page).build();
        view.add_top_bar(&adw::HeaderBar::new());
        let dialog = adw::Dialog::builder()
            .title(name.as_str())
            .content_width(560)
            .content_height(640)
            .child(&view)
            .build();
        dialog.present(Some(&window));
    });
}

/// The rows in display order. `None` stands for a value that could not be read.
fn rows(pid: i32, user: &str, details: &ProcessDetails) -> Vec<(String, Option<String>)> {
    let parent = match (details.parent, details.parent_name.as_str()) {
        (0, _) => None,
        (parent, "") => Some(parent.to_string()),
        (parent, name) => Some(format!("{name} ({parent})")),
    };
    let started = details
        .started
        .and_then(|seconds| glib::DateTime::from_unix_local(seconds).ok())
        .and_then(|time| time.format("%c").ok())
        .map(String::from);
    vec![
        (tr("PID"), Some(pid.to_string())),
        (tr("User"), Some(user.to_owned())),
        (tr("State"), Some(state(details.state))),
        (tr("Parent"), parent),
        (tr("Started"), started),
        (tr("Nice Value"), Some(details.nice.to_string())),
        (tr("Open Files"), details.open_files.map(|count| count.to_string())),
        (tr("Executable"), details.executable.clone()),
        (tr("Working Folder"), details.directory.clone()),
        (
            tr("Command Line"),
            Some(details.command.clone()).filter(|command| !command.is_empty()),
        ),
    ]
}

/// The state letters of proc(5) in words. An unknown letter is shown as it is.
fn state(letter: char) -> String {
    match letter {
        'R' => tr("Running"),
        'S' => tr("Sleeping"),
        'D' => tr("Waiting for I/O"),
        'I' => tr("Idle"),
        'T' | 't' => tr("Stopped"),
        'Z' => tr("Zombie"),
        other => other.to_string(),
    }
}
