// SPDX-License-Identifier: GPL-3.0-or-later

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::i18n::{tr, trf};
use crate::telemetry::autostart::{self, Entry, Locations};

struct Inner {
    root: gtk::Stack,
    group: adw::PreferencesGroup,
    toasts: adw::ToastOverlay,
    locations: Locations,
    rows: RefCell<Vec<adw::SwitchRow>>,
}

/// The programs that start at login, each with a switch.
#[derive(Clone)]
pub struct StartupPage(Rc<Inner>);

impl StartupPage {
    /// Failures to change an entry are reported through `toasts`.
    pub fn new(toasts: &adw::ToastOverlay) -> Self {
        let group = adw::PreferencesGroup::builder()
            .description(tr(
                "Programs that start when you log in. A change applies to your account only.",
            ))
            .build();
        let list = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&adw::Clamp::builder().maximum_size(720).child(&group).build())
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("system-run-symbolic")
            .title(tr("No Startup Programs"))
            .description(tr("Nothing is set to start when you log in."))
            .build();

        let root = gtk::Stack::builder().vexpand(true).build();
        root.add_named(&list, Some("list"));
        root.add_named(&empty, Some("empty"));
        Self(Rc::new(Inner {
            root,
            group,
            toasts: toasts.clone(),
            locations: Locations::current(),
            rows: RefCell::default(),
        }))
    }

    pub fn widget(&self) -> &gtk::Stack {
        &self.0.root
    }

    /// Reads the entries again, off the main loop, and rebuilds the list.
    pub fn refresh(&self) {
        let weak = Rc::downgrade(&self.0);
        let locations = self.0.locations.clone();
        glib::spawn_future_local(async move {
            let entries = gio::spawn_blocking(move || autostart::read(&locations))
                .await
                .unwrap_or_default();
            if let Some(inner) = weak.upgrade() {
                StartupPage(inner).show(entries);
            }
        });
    }

    fn show(&self, entries: Vec<Entry>) {
        let mut rows = self.0.rows.borrow_mut();
        for row in rows.drain(..) {
            self.0.group.remove(&row);
        }
        self.0
            .root
            .set_visible_child_name(if entries.is_empty() { "empty" } else { "list" });

        for entry in entries {
            // Names and commands come from files and are shown as plain text, not markup.
            let row = adw::SwitchRow::builder()
                .title(&entry.name)
                .subtitle(&entry.command)
                .subtitle_lines(1)
                .use_markup(false)
                .active(entry.enabled)
                .build();
            // Connected after the initial state is set, so only the user's changes arrive here.
            row.connect_active_notify({
                let weak = Rc::downgrade(&self.0);
                move |row| {
                    if let Some(inner) = weak.upgrade() {
                        StartupPage(inner).switch(&entry, row.is_active());
                    }
                }
            });
            self.0.group.add(&row);
            rows.push(row);
        }
    }

    fn switch(&self, entry: &Entry, enabled: bool) {
        if let Err(error) = autostart::set_enabled(&self.0.locations, &entry.id, enabled) {
            let message = trf("Could not change “{0}”: {1}", &[&entry.name, &error]);
            self.0.toasts.add_toast(adw::Toast::new(&message));
            // The switch has already moved, so the list is read again to show what is on disk.
            self.refresh();
        }
    }
}
