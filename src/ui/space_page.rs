// SPDX-License-Identifier: GPL-3.0-or-later

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib, pango};

use super::treemap::{Item, Treemap};
use crate::format;
use crate::telemetry::space::{Scan, SpaceNode};

/// How often the list follows a running scan.
const REFRESH_INTERVAL: Duration = Duration::from_millis(500);

mod imp {
    use std::cell::{Cell, RefCell};
    use std::sync::Arc;

    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    use crate::telemetry::space::SpaceNode;

    /// One row of the disk space table.
    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::SpaceObject)]
    pub struct SpaceObject {
        #[property(get, set)]
        name: RefCell<String>,
        #[property(get, set)]
        size: Cell<u64>,
        /// Part of the listed folder's size, 0–1.
        #[property(get, set)]
        share: Cell<f64>,
        pub node: RefCell<Option<Arc<SpaceNode>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SpaceObject {
        const NAME: &'static str = "PulseSpaceObject";
        type Type = super::SpaceObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for SpaceObject {}
}

glib::wrapper! {
    pub struct SpaceObject(ObjectSubclass<imp::SpaceObject>);
}

impl SpaceObject {
    fn new(node: &Arc<SpaceNode>) -> Self {
        let object: Self = glib::Object::new();
        object.imp().node.replace(Some(node.clone()));
        object
    }

    fn node(&self) -> Option<Arc<SpaceNode>> {
        self.imp().node.borrow().clone()
    }

    fn refresh(&self, node: &SpaceNode, total: u64) {
        let name = label(node);
        if self.name() != name {
            self.set_name(name);
        }
        let size = node.size();
        if self.size() != size {
            self.set_size(size);
        }
        let share = if total > 0 {
            (size as f64 / total as f64).min(1.0)
        } else {
            0.0
        };
        if self.share() != share {
            self.set_share(share);
        }
    }
}

/// What a node is called in the list and on the map.
fn label(node: &SpaceNode) -> String {
    if node.grouped > 0 {
        format!("{} smaller files", node.grouped)
    } else if node.unread() {
        format!("{} (not fully readable)", node.name)
    } else {
        node.name.clone()
    }
}

struct State {
    scan: Option<Scan>,
    /// The folders from the scan's root down to the one that is listed.
    trail: Vec<Arc<SpaceNode>>,
    targets: Vec<PathBuf>,
    objects: HashMap<*const SpaceNode, SpaceObject>,
    /// The listed folder's children in the order the map was given them.
    mapped: Vec<Arc<SpaceNode>>,
    /// Counts scans, so the timer of a replaced scan can tell that it is no longer wanted.
    generation: u32,
}

struct Inner {
    root: gtk::Box,
    targets: gtk::DropDown,
    target_names: gtk::StringList,
    scan_button: gtk::Button,
    up_button: gtk::Button,
    path: gtk::Label,
    status: gtk::Label,
    map: Treemap,
    /// Hidden until the first scan gives the map something to show.
    map_frame: gtk::Frame,
    view: gtk::ColumnView,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    state: RefCell<State>,
}

/// Shows which folders and files take up the space under a chosen folder.
/// A scan only starts when the user asks for one.
#[derive(Clone)]
pub struct SpacePage(Rc<Inner>);

impl SpacePage {
    pub fn new() -> Self {
        let state = State {
            scan: None,
            trail: Vec::new(),
            targets: vec![glib::home_dir(), PathBuf::from("/")],
            objects: HashMap::new(),
            mapped: Vec::new(),
            generation: 0,
        };
        let target_names = gtk::StringList::new(&["Home", "File System"]);
        let targets = gtk::DropDown::builder()
            .model(&target_names)
            .tooltip_text("Folder to scan")
            .build();
        let choose = gtk::Button::builder().label("Choose Folder…").build();
        let scan_button = gtk::Button::builder()
            .label("Scan")
            .css_classes(["suggested-action"])
            .build();
        let up_button = gtk::Button::builder()
            .icon_name("go-up-symbolic")
            .tooltip_text("Parent Folder")
            .sensitive(false)
            .build();
        let path = gtk::Label::builder()
            .label("Choose a folder and press Scan.")
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(pango::EllipsizeMode::Start)
            .build();
        let status = gtk::Label::builder().css_classes(["dim-label", "numeric"]).build();

        let bar = gtk::Box::builder().spacing(8).build();
        for widget in [
            up_button.upcast_ref::<gtk::Widget>(),
            path.upcast_ref(),
            status.upcast_ref(),
            targets.upcast_ref(),
            choose.upcast_ref(),
            scan_button.upcast_ref(),
        ] {
            bar.append(widget);
        }

        let store = gio::ListStore::new::<SpaceObject>();
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
        let name = name_column();
        let size = size_column();
        view.append_column(&name);
        view.append_column(&size);
        view.append_column(&share_column());
        sorted.set_sorter(view.sorter().as_ref());
        view.sort_by_column(Some(&size), gtk::SortType::Descending);

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&view)
            .build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        let map = Treemap::new();
        let map_frame = gtk::Frame::builder().child(map.widget()).visible(false).build();
        root.append(&bar);
        root.append(&map_frame);
        root.append(&gtk::Frame::builder().child(&scrolled).build());

        let page = Self(Rc::new(Inner {
            root,
            targets,
            target_names,
            scan_button,
            up_button,
            path,
            status,
            map,
            map_frame,
            view,
            store,
            selection,
            state: RefCell::new(state),
        }));
        page.connect(&choose);
        page
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.0.root
    }

    fn connect(&self, choose: &gtk::Button) {
        // The handlers hold weak references: the widgets they are attached to are owned by the page.
        let weak = Rc::downgrade(&self.0);
        let with_page = move |action: fn(&SpacePage)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    action(&SpacePage(inner));
                }
            }
        };

        self.0.scan_button.connect_clicked({
            let run = with_page(Self::toggle_scan);
            move |_| run()
        });
        self.0.up_button.connect_clicked({
            let run = with_page(Self::go_up);
            move |_| run()
        });
        choose.connect_clicked({
            let run = with_page(Self::choose_folder);
            move |_| run()
        });
        self.0.view.connect_activate({
            let weak = Rc::downgrade(&self.0);
            move |_, position| {
                let Some(page) = weak.upgrade().map(SpacePage) else {
                    return;
                };
                let row = page.0.selection.item(position).and_downcast::<SpaceObject>();
                page.open(row.and_then(|row| row.node()));
            }
        });
        self.0.map.connect_activated({
            let weak = Rc::downgrade(&self.0);
            move |index| {
                let Some(page) = weak.upgrade().map(SpacePage) else {
                    return;
                };
                let node = page.0.state.borrow().mapped.get(index).cloned();
                page.open(node);
            }
        });
    }

    fn toggle_scan(&self) {
        let running = self
            .0
            .state
            .borrow()
            .scan
            .as_ref()
            .is_some_and(|scan| !scan.is_finished());
        if running {
            if let Some(scan) = &self.0.state.borrow().scan {
                scan.cancel();
            }
            return;
        }
        let target = self
            .0
            .state
            .borrow()
            .targets
            .get(self.0.targets.selected() as usize)
            .cloned();
        if let Some(path) = target {
            self.start(path);
        }
    }

    fn start(&self, path: PathBuf) {
        let scan = match Scan::start(path) {
            Ok(scan) => scan,
            Err(error) => {
                self.0.status.set_label(&format!("Could not start the scan: {error}"));
                return;
            }
        };
        let generation = {
            let mut state = self.0.state.borrow_mut();
            state.trail = vec![scan.root.clone()];
            // Replacing the previous scan drops it, which cancels it if it is still running.
            state.scan = Some(scan);
            state.generation += 1;
            state.generation
        };
        self.0.map_frame.set_visible(true);
        self.0.scan_button.set_label("Cancel");
        self.0.scan_button.remove_css_class("suggested-action");
        self.list_folder();

        let weak = Rc::downgrade(&self.0);
        glib::timeout_add_local(REFRESH_INTERVAL, move || {
            let Some(inner) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let page = SpacePage(inner);
            if page.0.state.borrow().generation != generation {
                return glib::ControlFlow::Break;
            }
            let finished = page.0.state.borrow().scan.as_ref().is_none_or(Scan::is_finished);
            page.refresh();
            if !finished {
                return glib::ControlFlow::Continue;
            }
            page.0.scan_button.set_label("Scan");
            page.0.scan_button.add_css_class("suggested-action");
            glib::ControlFlow::Break
        });
    }

    fn choose_folder(&self) {
        let dialog = gtk::FileDialog::builder()
            .title("Choose a Folder to Scan")
            .modal(true)
            .build();
        let window = self.0.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(&self.0);
        dialog.select_folder(window.as_ref(), gio::Cancellable::NONE, move |result| {
            // An error here means the dialog was dismissed.
            let (Ok(folder), Some(inner)) = (result, weak.upgrade()) else {
                return;
            };
            let Some(path) = folder.path() else { return };
            let page = SpacePage(inner);
            let index = {
                let mut state = page.0.state.borrow_mut();
                state
                    .targets
                    .iter()
                    .position(|target| *target == path)
                    .unwrap_or_else(|| {
                        page.0.target_names.append(&path.to_string_lossy());
                        state.targets.push(path.clone());
                        state.targets.len() - 1
                    })
            };
            page.0.targets.set_selected(index as u32);
            page.start(path);
        });
    }

    /// Lists a folder that was activated in the table or on the map. Files stay where they are.
    fn open(&self, node: Option<Arc<SpaceNode>>) {
        if let Some(node) = node.filter(|node| node.is_folder) {
            self.0.state.borrow_mut().trail.push(node);
            self.list_folder();
        }
    }

    fn go_up(&self) {
        {
            let mut state = self.0.state.borrow_mut();
            if state.trail.len() < 2 {
                return;
            }
            state.trail.pop();
        }
        self.list_folder();
    }

    /// Shows the folder at the end of the trail from scratch.
    fn list_folder(&self) {
        self.0.state.borrow_mut().objects.clear();
        self.0.store.remove_all();
        self.refresh();
        // Start at the largest entries whatever position the previous folder was left at.
        if self.0.store.n_items() > 0 {
            self.0.view.scroll_to(0, None, gtk::ListScrollFlags::NONE, None);
        }
    }

    /// Brings rows, path and status in line with the scan. Rows keep their objects,
    /// so the list holds its scroll position and selection while sizes grow.
    fn refresh(&self) {
        let mut state = self.0.state.borrow_mut();
        let Some(folder) = state.trail.last().cloned() else {
            return;
        };
        let total = folder.size();
        let children = folder.children();

        if state.objects.len() != children.len() {
            // A folder's children are published once, so a different count means they have just arrived.
            for child in &children {
                state.objects.entry(Arc::as_ptr(child)).or_insert_with(|| {
                    let object = SpaceObject::new(child);
                    self.0.store.append(&object);
                    object
                });
            }
        }
        for child in &children {
            if let Some(object) = state.objects.get(&Arc::as_ptr(child)) {
                object.refresh(child, total);
            }
        }
        if let Some(sorter) = self.0.view.sorter() {
            sorter.changed(gtk::SorterChange::Different);
        }
        let items = children.iter().map(|child| Item {
            label: label(child),
            size: child.size(),
            folder: child.is_folder,
        });
        self.0.map.set_items(items.collect());
        state.mapped = children;

        let names: Vec<&str> = state.trail.iter().map(|node| node.name.trim_end_matches('/')).collect();
        let path = names.join("/");
        self.0.path.set_label(if path.is_empty() { "/" } else { &path });
        self.0.up_button.set_sensitive(state.trail.len() > 1);

        let progress = match &state.scan {
            Some(scan) if !scan.is_finished() => " · Scanning…",
            Some(scan) if scan.is_cancelled() => " · Cancelled",
            _ if folder.unread() => " · Some folders could not be read",
            _ => "",
        };
        self.0
            .status
            .set_label(&format!("{}{progress}", format::bytes(total as f64)));
    }
}

fn property(name: &str) -> gtk::PropertyExpression {
    gtk::PropertyExpression::new(SpaceObject::static_type(), gtk::Expression::NONE, name)
}

fn name_column() -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::Middle)
            .build();
        item.set_child(Some(&label));
        let name = item.property_expression("item").chain_property::<SpaceObject>("name");
        name.bind(&label, "label", gtk::Widget::NONE);
        name.bind(&label, "tooltip-text", gtk::Widget::NONE);
    });
    gtk::ColumnViewColumn::builder()
        .title("Name")
        .factory(&factory)
        .sorter(&gtk::StringSorter::new(Some(property("name"))))
        .expand(true)
        .resizable(true)
        .build()
}

fn size_column() -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::builder().xalign(1.0).css_classes(["numeric"]).build();
        item.set_child(Some(&label));
        item.property_expression("item")
            .chain_property::<SpaceObject>("size")
            .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, size: u64| format::bytes(
                size as f64
            )))
            .bind(&label, "label", gtk::Widget::NONE);
    });
    gtk::ColumnViewColumn::builder()
        .title("Size on Disk")
        .factory(&factory)
        .sorter(&gtk::NumericSorter::new(Some(property("size"))))
        .resizable(true)
        .build()
}

fn share_column() -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let bar = gtk::ProgressBar::builder()
            .valign(gtk::Align::Center)
            .width_request(120)
            .build();
        let label = gtk::Label::builder()
            .xalign(1.0)
            .width_chars(7)
            .css_classes(["numeric"])
            .build();
        let cell = gtk::Box::builder().spacing(8).build();
        cell.append(&bar);
        cell.append(&label);
        item.set_child(Some(&cell));
        let share = item.property_expression("item").chain_property::<SpaceObject>("share");
        share.bind(&bar, "fraction", gtk::Widget::NONE);
        share
            .chain_closure::<String>(glib::closure!(|_: Option<glib::Object>, share: f64| {
                format::percent(Some(100.0 * share))
            }))
            .bind(&label, "label", gtk::Widget::NONE);
    });
    gtk::ColumnViewColumn::builder()
        .title("Share")
        .factory(&factory)
        .sorter(&gtk::NumericSorter::new(Some(property("share"))))
        .build()
}
