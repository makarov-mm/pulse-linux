// SPDX-License-Identifier: GPL-3.0-or-later

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use gtk::prelude::*;
use gtk::{gio, glib};

use super::table::{number_column, text_column};
use crate::telemetry::sockets::Socket;

mod imp {
    use std::cell::{Cell, RefCell};

    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    /// One row of the connections table.
    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::ConnectionObject)]
    pub struct ConnectionObject {
        #[property(get, set)]
        process: RefCell<String>,
        /// Negative while the owner is unknown, which sorts those rows together.
        #[property(get, set)]
        pid: Cell<i32>,
        #[property(get, set)]
        pid_text: RefCell<String>,
        #[property(get, set)]
        protocol: RefCell<String>,
        #[property(get, set)]
        local: RefCell<String>,
        #[property(get, set)]
        remote: RefCell<String>,
        #[property(get, set)]
        state: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ConnectionObject {
        const NAME: &'static str = "PulseConnectionObject";
        type Type = super::ConnectionObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for ConnectionObject {}
}

glib::wrapper! {
    pub struct ConnectionObject(ObjectSubclass<imp::ConnectionObject>);
}

impl ConnectionObject {
    fn new(socket: &Socket) -> Self {
        let object: Self = glib::Object::builder()
            .property("protocol", socket.protocol.name())
            .property("local", socket.local.to_string())
            .property(
                "remote",
                socket.remote.map(|remote| remote.to_string()).unwrap_or_default(),
            )
            .build();
        object.refresh(socket);
        object
    }

    /// The addresses of a socket never change. Its state and the process found for it can.
    fn refresh(&self, socket: &Socket) {
        if self.state() != socket.state {
            self.set_state(socket.state);
        }
        let pid = socket.pid.unwrap_or(-1);
        if self.pid() != pid || self.pid_text().is_empty() {
            self.set_pid(pid);
            self.set_pid_text(socket.pid.map_or_else(|| "—".to_owned(), |pid| pid.to_string()));
        }
        if self.process() != socket.process {
            self.set_process(socket.process.as_str());
        }
    }
}

/// What tells one socket from another across refreshes.
type Key = (
    crate::telemetry::sockets::Protocol,
    std::net::SocketAddr,
    Option<std::net::SocketAddr>,
    u64,
);

/// The TCP and UDP sockets that are open, with the process that holds each.
pub struct ConnectionsPage {
    root: gtk::Box,
    view: gtk::ColumnView,
    store: gio::ListStore,
    note: gtk::Label,
    objects: RefCell<HashMap<Key, ConnectionObject>>,
}

impl ConnectionsPage {
    pub fn new() -> Self {
        let store = gio::ListStore::new::<ConnectionObject>();
        let sorted = gtk::SortListModel::new(Some(store.clone()), None::<gtk::Sorter>);
        let selection = gtk::SingleSelection::builder()
            .model(&sorted)
            .autoselect(false)
            .can_unselect(true)
            .build();
        let view = gtk::ColumnView::builder()
            .model(&selection)
            .css_classes(["data-table"])
            .build();

        let process = text_column::<ConnectionObject>("Process", "process", true);
        view.append_column(&process);
        view.append_column(&number_column::<ConnectionObject>("PID", "pid-text", "pid"));
        view.append_column(&text_column::<ConnectionObject>("Protocol", "protocol", false));
        view.append_column(&text_column::<ConnectionObject>("Local Address", "local", true));
        view.append_column(&text_column::<ConnectionObject>("Remote Address", "remote", true));
        view.append_column(&text_column::<ConnectionObject>("State", "state", false));
        sorted.set_sorter(view.sorter().as_ref());
        view.sort_by_column(Some(&process), gtk::SortType::Ascending);

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&view)
            .build();
        let note = gtk::Label::builder()
            .label("Sockets of other users' processes are listed without an owner unless Pulse runs as root.")
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .css_classes(["caption", "dim-label"])
            .build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        root.append(&gtk::Frame::builder().child(&scrolled).build());
        root.append(&note);
        Self {
            root,
            view,
            store,
            note,
            objects: RefCell::default(),
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Brings the rows in line with a new reading. A row keeps its object while its socket exists.
    pub fn update(&self, sockets: &[Socket]) {
        let mut objects = self.objects.borrow_mut();
        let mut open = HashSet::with_capacity(sockets.len());
        for socket in sockets {
            let key = (socket.protocol, socket.local, socket.remote, socket.inode);
            // A reading can list one socket twice while it changes hands. The first entry stands.
            if !open.insert(key) {
                continue;
            }
            match objects.get(&key) {
                Some(object) => object.refresh(socket),
                None => {
                    let object = ConnectionObject::new(socket);
                    self.store.append(&object);
                    objects.insert(key, object);
                }
            }
        }
        if objects.len() != open.len() {
            let closed: HashSet<ConnectionObject> = objects
                .iter()
                .filter(|(key, _)| !open.contains(key))
                .map(|(_, object)| object.clone())
                .collect();
            objects.retain(|key, _| open.contains(key));
            self.store.retain(|object| {
                object
                    .downcast_ref::<ConnectionObject>()
                    .is_some_and(|row| !closed.contains(row))
            });
        }
        if let Some(sorter) = self.view.sorter() {
            sorter.changed(gtk::SorterChange::Different);
        }
        self.note.set_visible(sockets.iter().any(|socket| socket.pid.is_none()));
    }
}
