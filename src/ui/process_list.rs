// SPDX-License-Identifier: GPL-3.0-or-later

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib, pango};

use crate::format;
use crate::telemetry::{ProcessKey, ProcessSample};

mod imp {
    use std::cell::{Cell, RefCell};

    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    /// One row of the process table. The table reads it through its properties,
    /// so a changed value redraws only the cell that shows it.
    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::ProcessObject)]
    pub struct ProcessObject {
        #[property(get, set)]
        pid: Cell<i32>,
        #[property(get, set)]
        name: RefCell<String>,
        /// Percent of all processors. Negative while there is no measurement yet.
        #[property(get, set)]
        cpu: Cell<f64>,
        #[property(get, set)]
        memory: Cell<u64>,
        #[property(get, set)]
        threads: Cell<u32>,
        pub start_time: Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProcessObject {
        const NAME: &'static str = "PulseProcessObject";
        type Type = super::ProcessObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for ProcessObject {}
}

glib::wrapper! {
    pub struct ProcessObject(ObjectSubclass<imp::ProcessObject>);
}

impl ProcessObject {
    fn new(sample: &ProcessSample) -> Self {
        let object: Self = glib::Object::builder()
            .property("pid", sample.key.pid)
            .property("name", &sample.name)
            .build();
        object.imp().start_time.set(sample.key.start_time);
        object.refresh(sample);
        object
    }

    pub fn key(&self) -> ProcessKey {
        ProcessKey { pid: self.pid(), start_time: self.imp().start_time.get() }
    }

    /// Only values that changed are set, so unchanged cells are not redrawn.
    fn refresh(&self, sample: &ProcessSample) {
        let cpu = sample.cpu.unwrap_or(-1.0);
        if self.cpu() != cpu {
            self.set_cpu(cpu);
        }
        if self.memory() != sample.memory {
            self.set_memory(sample.memory);
        }
        if self.threads() != sample.threads {
            self.set_threads(sample.threads);
        }
        // A process can rename itself (prctl, exec).
        if self.name() != sample.name {
            self.set_name(sample.name.as_str());
        }
    }
}

/// The sortable, searchable table of processes.
pub struct ProcessList {
    root: gtk::ScrolledWindow,
    view: gtk::ColumnView,
    store: gio::ListStore,
    filter: gtk::StringFilter,
    selection: gtk::SingleSelection,
    objects: RefCell<HashMap<ProcessKey, ProcessObject>>,
}

impl ProcessList {
    pub fn new() -> Self {
        let store = gio::ListStore::new::<ProcessObject>();
        let filter = gtk::StringFilter::builder()
            .expression(property("name"))
            .ignore_case(true)
            .match_mode(gtk::StringFilterMatchMode::Substring)
            .build();
        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        let sorted = gtk::SortListModel::new(Some(filtered), None::<gtk::Sorter>);
        let selection = gtk::SingleSelection::builder()
            .model(&sorted)
            .autoselect(false)
            .can_unselect(true)
            .build();
        let view = gtk::ColumnView::builder()
            .model(&selection)
            .show_column_separators(false)
            .css_classes(["data-table"])
            .build();

        let name = column("Name", true, &gtk::StringSorter::new(Some(property("name"))), |item| {
            item.chain_property::<ProcessObject>("name").upcast()
        });
        let pid = column("PID", false, &numeric_sorter("pid"), |item| {
            item.chain_property::<ProcessObject>("pid")
                .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, pid: i32| pid.to_string()))
                .upcast()
        });
        let cpu = column("CPU", false, &numeric_sorter("cpu"), |item| {
            item.chain_property::<ProcessObject>("cpu")
                .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, cpu: f64| {
                    format::percent((cpu >= 0.0).then_some(cpu))
                }))
                .upcast()
        });
        let memory = column("Memory", false, &numeric_sorter("memory"), |item| {
            item.chain_property::<ProcessObject>("memory")
                .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, bytes: u64| {
                    format::bytes(bytes as f64)
                }))
                .upcast()
        });
        let threads = column("Threads", false, &numeric_sorter("threads"), |item| {
            item.chain_property::<ProcessObject>("threads")
                .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, threads: u32| threads.to_string()))
                .upcast()
        });
        for column in [&name, &pid, &cpu, &memory, &threads] {
            view.append_column(column);
        }

        // The view's own sorter follows the column headers the user clicks.
        sorted.set_sorter(view.sorter().as_ref());
        view.sort_by_column(Some(&cpu), gtk::SortType::Descending);

        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&view)
            .build();
        Self { root, view, store, filter, selection, objects: RefCell::default() }
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    /// Brings the rows in line with a new sample. A row keeps its object for as long as
    /// its process lives, which keeps the selection and scroll position stable.
    pub fn update(&self, processes: &[ProcessSample]) {
        let mut objects = self.objects.borrow_mut();
        let mut alive = HashSet::with_capacity(processes.len());
        for sample in processes {
            alive.insert(sample.key);
            match objects.get(&sample.key) {
                Some(object) => object.refresh(sample),
                None => {
                    let object = ProcessObject::new(sample);
                    self.store.append(&object);
                    objects.insert(sample.key, object);
                }
            }
        }
        if objects.len() != alive.len() {
            objects.retain(|key, _| alive.contains(key));
            self.store.retain(|object| {
                object.downcast_ref::<ProcessObject>().is_some_and(|process| alive.contains(&process.key()))
            });
        }
        // Sort models do not watch item properties, so the new values are announced here.
        if let Some(sorter) = self.view.sorter() {
            sorter.changed(gtk::SorterChange::Different);
        }
    }

    pub fn set_search(&self, text: &str) {
        self.filter.set_search(Some(text));
    }

    pub fn selected(&self) -> Option<ProcessObject> {
        self.selection.selected_item().and_downcast()
    }

    pub fn connect_selection_changed(&self, callback: impl Fn(Option<ProcessObject>) + 'static) {
        self.selection.connect_selected_item_notify(move |selection| {
            callback(selection.selected_item().and_downcast());
        });
    }
}

fn property(name: &str) -> gtk::PropertyExpression {
    gtk::PropertyExpression::new(ProcessObject::static_type(), gtk::Expression::NONE, name)
}

fn numeric_sorter(name: &str) -> gtk::NumericSorter {
    gtk::NumericSorter::new(Some(property(name)))
}

/// A text column. `text` turns the expression for a row's `ProcessObject` into the cell's string.
fn column(
    title: &str,
    is_name: bool,
    sorter: &impl IsA<gtk::Sorter>,
    text: impl Fn(gtk::PropertyExpression) -> gtk::Expression + 'static,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
        let label = gtk::Label::builder()
            .xalign(if is_name { 0.0 } else { 1.0 })
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        if !is_name {
            label.add_css_class("numeric");
        }
        item.set_child(Some(&label));
        text(item.property_expression("item")).bind(&label, "label", gtk::Widget::NONE);
    });
    gtk::ColumnViewColumn::builder()
        .title(title)
        .factory(&factory)
        .sorter(sorter)
        .expand(is_name)
        .resizable(true)
        .build()
}
