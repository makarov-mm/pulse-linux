// SPDX-License-Identifier: GPL-3.0-or-later

use std::cell::{Cell as StdCell, RefCell};
use std::collections::{HashMap, HashSet};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib, pango};

use crate::format;
use crate::i18n::tr;
use crate::telemetry::{ProcessKey, ProcessSample};

mod imp {
    use std::cell::{Cell, RefCell};

    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use gtk::{gio, glib};

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
        #[property(get, set)]
        user: RefCell<String>,
        /// The command line, shortened to what a tooltip can show.
        #[property(get, set)]
        command: RefCell<String>,
        /// Bytes per second read and written. Negative while it is unknown or unreadable.
        #[property(get, set)]
        disk: Cell<f64>,
        #[property(get, set)]
        kernel: Cell<bool>,
        /// Everything the search box matches against.
        #[property(get, set)]
        pub(super) search: RefCell<String>,
        /// Whether the table is a tree. Rows without children then leave room for the arrow of their siblings.
        #[property(get, set)]
        in_tree: Cell<bool>,
        pub start_time: Cell<u64>,
        pub parent: Cell<i32>,
        /// The processes shown below this one in the tree.
        pub children: RefCell<Option<gio::ListStore>>,
        /// The list this row is in: the top level or the children of another row.
        pub container: RefCell<Option<gio::ListStore>>,
        /// Whether the row was given to the tree with children. A tree row cannot change that afterwards.
        pub expandable: Cell<bool>,
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
            .property("kernel", sample.kernel)
            .build();
        object.imp().start_time.set(sample.key.start_time);
        object.imp().children.replace(Some(gio::ListStore::new::<Self>()));
        object.refresh(sample);
        object
    }

    pub fn key(&self) -> ProcessKey {
        ProcessKey {
            pid: self.pid(),
            start_time: self.imp().start_time.get(),
        }
    }

    fn children(&self) -> gio::ListStore {
        self.imp().children.borrow().clone().expect("set at construction")
    }

    /// Takes the row out of the list it is in.
    fn detach(&self) {
        if let Some(container) = self.imp().container.take()
            && let Some(position) = container.find(self)
        {
            container.remove(position);
        }
    }

    /// Only values that changed are set, so unchanged cells are not redrawn.
    fn refresh(&self, sample: &ProcessSample) -> bool {
        // A process whose parent exits is adopted by another one.
        self.imp().parent.set(sample.parent);
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
        let disk = sample.disk.unwrap_or(-1.0);
        if self.disk() != disk {
            self.set_disk(disk);
        }
        // Compare the full command, including the part omitted from the tooltip.
        let search = format!("{} {} {} {}", sample.name, sample.key.pid, sample.user, sample.command);
        let name_changed = self.name() != sample.name;
        let user_changed = self.user() != sample.user.as_ref();
        if !name_changed && !user_changed && *self.imp().search.borrow() == search {
            return false;
        }
        if name_changed {
            self.set_name(sample.name.as_str());
        }
        if user_changed {
            self.set_user(sample.user.as_ref());
        }
        self.set_command(tooltip(&sample.command));
        self.set_search(search);
        true
    }
}

/// Longest command line shown in a tooltip. GTK lays a tooltip out as one window,
/// and a command of many kilobytes makes that window larger than a renderer can draw.
const TOOLTIP_CHARS: usize = 400;

fn tooltip(command: &str) -> String {
    let mut text: String = command
        .chars()
        .take(TOOLTIP_CHARS)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if command.chars().nth(TOOLTIP_CHARS).is_some() {
        text.push('…');
    }
    text
}

/// The sortable, searchable table of processes.
pub struct ProcessList {
    root: gtk::ScrolledWindow,
    view: gtk::ColumnView,
    /// Top-level processes in the tree.
    store: gio::ListStore,
    all: gio::ListStore,
    flat_rows: gtk::TreeListModel,
    tree_rows: gtk::TreeListModel,
    filtered: gtk::FilterListModel,
    tree: StdCell<bool>,
    search: gtk::StringFilter,
    /// Hides kernel threads while it is part of `filters`.
    user_only: gtk::BoolFilter,
    filters: gtk::EveryFilter,
    selection: gtk::SingleSelection,
    objects: RefCell<HashMap<ProcessKey, ProcessObject>>,
}

impl ProcessList {
    pub fn new() -> Self {
        let store = gio::ListStore::new::<ProcessObject>();
        let search = gtk::StringFilter::builder()
            .expression(row_property("search"))
            .ignore_case(true)
            .match_mode(gtk::StringFilterMatchMode::Substring)
            .build();
        let user_only = gtk::BoolFilter::builder()
            .expression(row_property("kernel"))
            .invert(true)
            .build();
        let filters = gtk::EveryFilter::new();
        filters.append(search.clone());
        filters.append(user_only.clone());
        // Search uses every process, while the separate tree keeps its expansion state.
        let all = gio::ListStore::new::<ProcessObject>();
        let flat_rows = gtk::TreeListModel::new(all.clone(), false, false, |_| None);
        let tree_rows = gtk::TreeListModel::new(store.clone(), false, true, |item| {
            let process = item.downcast_ref::<ProcessObject>()?;
            process.imp().expandable.get().then(|| process.children().upcast())
        });
        let filtered = gtk::FilterListModel::new(Some(flat_rows.clone()), Some(filters.clone()));
        let sorted = gtk::SortListModel::new(Some(filtered.clone()), None::<gtk::Sorter>);
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

        let text = |name: &'static str| {
            move |item: gtk::PropertyExpression| item.chain_property::<ProcessObject>(name).upcast()
        };
        let name = column(
            &tr("Name"),
            Cell::Wide,
            &string_sorter("name"),
            Some("command"),
            text("name"),
        );
        let pid = column(&tr("PID"), Cell::Number, &numeric_sorter("pid"), None, |item| {
            item.chain_property::<ProcessObject>("pid")
                .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, pid: i32| pid.to_string()))
                .upcast()
        });
        let user = column(&tr("User"), Cell::Text, &string_sorter("user"), None, text("user"));
        let cpu = column(&tr("CPU"), Cell::Number, &numeric_sorter("cpu"), None, |item| {
            item.chain_property::<ProcessObject>("cpu")
                .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, cpu: f64| {
                    format::percent((cpu >= 0.0).then_some(cpu))
                }))
                .upcast()
        });
        let memory = column(&tr("Memory"), Cell::Number, &numeric_sorter("memory"), None, |item| {
            item.chain_property::<ProcessObject>("memory")
                .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, bytes: u64| {
                    format::bytes(bytes as f64)
                }))
                .upcast()
        });
        let disk = column(&tr("Disk"), Cell::Number, &numeric_sorter("disk"), None, |item| {
            item.chain_property::<ProcessObject>("disk")
                .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, rate: f64| {
                    format::rate((rate >= 0.0).then_some(rate))
                }))
                .upcast()
        });
        let threads = column(&tr("Threads"), Cell::Number, &numeric_sorter("threads"), None, |item| {
            item.chain_property::<ProcessObject>("threads")
                .chain_closure::<String>(glib::closure!(
                    |_: Option<glib::Object>, threads: u32| threads.to_string()
                ))
                .upcast()
        });
        for column in [&name, &pid, &user, &cpu, &memory, &disk, &threads] {
            view.append_column(column);
        }

        // The view's own sorter follows the column headers the user clicks. Wrapped like this it orders
        // the children of each row among themselves and keeps them below their parent.
        sorted.set_sorter(Some(&gtk::TreeListRowSorter::new(view.sorter())));
        view.sort_by_column(Some(&cpu), gtk::SortType::Descending);

        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .child(&view)
            .build();
        Self {
            root,
            view,
            store,
            all,
            flat_rows,
            tree_rows,
            filtered,
            tree: StdCell::new(false),
            search,
            user_only,
            filters,
            selection,
            objects: RefCell::default(),
        }
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    /// Brings the rows in line with a new sample. A row keeps its object for as long as
    /// its process lives, which keeps the selection and scroll position stable.
    pub fn update(&self, processes: &[ProcessSample]) {
        let mut objects = self.objects.borrow_mut();
        let mut alive = HashSet::with_capacity(processes.len());
        let mut search_changed = false;
        let mut added = Vec::new();
        for sample in processes {
            alive.insert(sample.key);
            match objects.get(&sample.key) {
                Some(object) => search_changed |= object.refresh(sample),
                None => {
                    let object = ProcessObject::new(sample);
                    added.push(object.clone());
                    objects.insert(sample.key, object);
                }
            }
        }
        if !added.is_empty() {
            self.all.extend_from_slice(&added);
        }
        if objects.len() != alive.len() {
            self.all.retain(|object| {
                object
                    .downcast_ref::<ProcessObject>()
                    .is_some_and(|row| alive.contains(&row.key()))
            });
            objects.retain(|key, object| {
                let keep = alive.contains(key);
                if !keep {
                    object.detach();
                }
                keep
            });
        }
        let selected = self.selected().map(|process| process.key());
        self.arrange(&objects);
        if search_changed && self.search.search().is_some_and(|text| !text.is_empty()) {
            self.search.changed(gtk::FilterChange::Different);
        }
        self.reselect(selected);
        // Sort models do not watch item properties, so the new values are announced here.
        if let Some(sorter) = self.view.sorter() {
            sorter.changed(gtk::SorterChange::Different);
        }
    }

    pub fn set_search(&self, text: &str) {
        let selected = self.selected().map(|process| process.key());
        self.search.set_search(Some(text));
        self.choose_model();
        self.reselect(selected);
    }

    pub fn set_show_kernel_threads(&self, show: bool) {
        let hidden = self.filters.n_items() > 1;
        if show && hidden {
            self.filters.remove(1);
        } else if !show && !hidden {
            self.filters.append(self.user_only.clone());
        }
    }

    /// Shows the processes below their parents, or as a flat list.
    pub fn set_tree(&self, tree: bool) {
        if self.tree.replace(tree) != tree {
            let selected = self.selected().map(|process| process.key());
            self.arrange(&self.objects.borrow());
            self.choose_model();
            self.reselect(selected);
        }
    }

    fn showing_tree(&self) -> bool {
        self.tree.get() && self.search.search().is_none_or(|text| text.is_empty())
    }

    fn choose_model(&self) {
        let tree = self.showing_tree();
        let model = if tree { &self.tree_rows } else { &self.flat_rows };
        if self.filtered.model().as_ref() != Some(model.upcast_ref()) {
            for row in self.objects.borrow().values() {
                row.set_in_tree(tree);
            }
            self.filtered.set_model(Some(model));
        }
    }

    /// Puts every row into the list it belongs to: the children of its parent in the tree,
    /// the top level otherwise. Rows already in place are left alone.
    fn arrange(&self, objects: &HashMap<ProcessKey, ProcessObject>) {
        if !self.tree.get() {
            return;
        }
        let tree = self.showing_tree();
        let mut parents: HashMap<i32, (&ProcessObject, bool)> =
            objects.values().map(|object| (object.pid(), (object, false))).collect();
        for object in objects.values() {
            if let Some((_, has_children)) = parents.get_mut(&object.imp().parent.get()) {
                *has_children = true;
            }
        }
        for object in objects.values() {
            if object.in_tree() != tree {
                object.set_in_tree(tree);
            }
            let state = object.imp();
            let container = match parents.get(&state.parent.get()) {
                Some((parent, _)) => parent.children(),
                None => self.store.clone(),
            };
            let expandable = parents
                .get(&object.pid())
                .is_some_and(|(_, has_children)| *has_children);
            // A tree row is asked for its children once, so a row that gains its first child
            // or loses its last one has to be inserted anew.
            if state.container.borrow().as_ref() != Some(&container) || state.expandable.get() != expandable {
                object.detach();
                state.expandable.set(expandable);
                container.append(object);
                state.container.replace(Some(container));
            }
        }
    }

    /// Selects the row of `key` again after it was moved to another list, which drops the selection.
    fn reselect(&self, key: Option<ProcessKey>) {
        let Some(key) = key else { return };
        if self.selected().is_some_and(|process| process.key() == key) {
            return;
        }
        let position = (0..self.selection.n_items())
            .find(|&position| process_at(&self.selection, position).is_some_and(|process| process.key() == key));
        if let Some(position) = position {
            self.selection.set_selected(position);
        }
    }

    pub fn selected(&self) -> Option<ProcessObject> {
        process_of(self.selection.selected_item())
    }

    /// Calls `callback` when a row is activated: double-clicked, or Enter is pressed on it.
    pub fn connect_activated(&self, callback: impl Fn(ProcessObject) + 'static) {
        let selection = self.selection.clone();
        self.view.connect_activate(move |_, position| {
            if let Some(process) = process_at(&selection, position) {
                callback(process);
            }
        });
    }

    pub fn connect_selection_changed(&self, callback: impl Fn(Option<ProcessObject>) + 'static) {
        self.selection.connect_selected_item_notify(move |selection| {
            callback(process_of(selection.selected_item()));
        });
    }
}

/// The process shown by a row of the table's model.
fn process_of(row: Option<glib::Object>) -> Option<ProcessObject> {
    row.and_downcast::<gtk::TreeListRow>()?.item().and_downcast()
}

fn process_at(selection: &gtk::SingleSelection, position: u32) -> Option<ProcessObject> {
    process_of(selection.item(position))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(pid: i32, parent: i32, name: &str) -> ProcessSample {
        ProcessSample {
            key: ProcessKey { pid, start_time: 1 },
            parent,
            name: name.into(),
            user: "user".into(),
            command: name.into(),
            kernel: false,
            cpu: None,
            memory: 0,
            disk: None,
            threads: 1,
        }
    }

    #[test]
    fn renamed_process_updates_the_search_text_and_tooltip() {
        let mut sample = sample(41, 1, "old");
        let object = ProcessObject::new(&sample);
        sample.name = "new".into();
        sample.user = "other".into();
        sample.command = format!("new {} first", "x".repeat(500));
        assert!(object.refresh(&sample));
        assert_eq!(object.name(), "new");
        assert_eq!(object.user(), "other");
        assert_eq!(object.command(), tooltip(&sample.command));
        sample.command.push_str(" second");
        assert!(object.refresh(&sample));
        assert!(object.search().ends_with("first second"));
        assert!(!object.refresh(&sample));
    }

    #[test]
    #[ignore = "requires a GTK display"]
    fn search_finds_collapsed_children_and_refilters_live_changes() {
        gtk::init().unwrap();
        let list = ProcessList::new();
        let parent = sample(41, 0, "parent");
        let mut child = sample(42, 41, "child");
        list.update(&[parent.clone(), child.clone()]);
        list.set_tree(true);
        let branch = list.tree_rows.row(0).unwrap();
        branch.set_expanded(false);
        assert_eq!(list.selection.n_items(), 1);
        list.set_search("42");
        assert_eq!(process_at(&list.selection, 0).unwrap().pid(), 42);
        list.set_search("fresh-argument");
        assert_eq!(list.selection.n_items(), 0);
        child.command = "program fresh-argument".into();
        list.update(&[parent, child]);
        assert_eq!(process_at(&list.selection, 0).unwrap().pid(), 42);
        list.set_search("");
        assert!(!branch.is_expanded());
        assert_eq!(list.selection.n_items(), 1);
    }
}

fn property(name: &str) -> gtk::PropertyExpression {
    gtk::PropertyExpression::new(ProcessObject::static_type(), gtk::Expression::NONE, name)
}

/// `property` for filters, which see the tree rows instead of the processes.
fn row_property(name: &str) -> gtk::PropertyExpression {
    gtk::PropertyExpression::new(gtk::TreeListRow::static_type(), gtk::Expression::NONE, "item")
        .chain_property::<ProcessObject>(name)
}

fn numeric_sorter(name: &str) -> gtk::NumericSorter {
    gtk::NumericSorter::new(Some(property(name)))
}

fn string_sorter(name: &str) -> gtk::StringSorter {
    gtk::StringSorter::new(Some(property(name)))
}

/// How a column lays out its text.
#[derive(Clone, Copy, PartialEq)]
enum Cell {
    /// Left-aligned and given the spare width.
    Wide,
    Text,
    /// Right-aligned with tabular digits.
    Number,
}

/// A text column. `text` turns the expression for a row's `ProcessObject` into the cell's string,
/// and `tooltip` names a property shown when the pointer rests on the cell.
fn column(
    title: &str,
    cell: Cell,
    sorter: &impl IsA<gtk::Sorter>,
    tooltip: Option<&'static str>,
    text: impl Fn(gtk::PropertyExpression) -> gtk::Expression + 'static,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::builder()
            .xalign(if cell == Cell::Number { 1.0 } else { 0.0 })
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        if cell == Cell::Number {
            label.add_css_class("numeric");
        }
        if cell == Cell::Wide {
            // The first column draws the tree: indentation and the arrow that folds a row's children.
            let expander = gtk::TreeExpander::builder().child(&label).build();
            item.set_child(Some(&expander));
            item.property_expression("item")
                .bind(&expander, "list-row", gtk::Widget::NONE);
            item.property_expression("item")
                .chain_property::<gtk::TreeListRow>("item")
                .chain_property::<ProcessObject>("in-tree")
                .bind(&expander, "indent-for-icon", gtk::Widget::NONE);
        } else {
            item.set_child(Some(&label));
        }
        let process = || {
            item.property_expression("item")
                .chain_property::<gtk::TreeListRow>("item")
        };
        text(process()).bind(&label, "label", gtk::Widget::NONE);
        if let Some(tooltip) = tooltip {
            process()
                .chain_property::<ProcessObject>(tooltip)
                .bind(&label, "tooltip-text", gtk::Widget::NONE);
        }
    });
    gtk::ColumnViewColumn::builder()
        .title(title)
        .factory(&factory)
        .sorter(sorter)
        .expand(cell == Cell::Wide)
        .resizable(true)
        .build()
}
