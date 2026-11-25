// SPDX-License-Identifier: GPL-3.0-or-later

use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::format;
use crate::i18n::{tr, trf, trn};
use crate::settings;
use crate::telemetry::{self, Frame, Sampler, sockets};
use crate::ui::{
    ConnectionsPage, DevicesPage, MetricCard, ProcessList, ProcessObject, SpacePage, StartupPage,
    present_process_details,
};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const PROCESSES_PAGE: &str = "processes";
const CONNECTIONS_PAGE: &str = "connections";
const STARTUP_PAGE: &str = "startup";
/// Below this width the tabs move to the bottom and the cards form two rows.
const NARROW_WIDTH: f64 = 1000.0;
/// Reading the socket tables walks every process's descriptors, so it runs less often than sampling.
const SOCKET_INTERVAL: Duration = Duration::from_secs(2);

/// The parts of the window that every sample updates.
struct Live {
    cpu: MetricCard,
    memory: MetricCard,
    disk: MetricCard,
    network: MetricCard,
    processes: Rc<ProcessList>,
    devices: DevicesPage,
}

pub fn build(app: &adw::Application) -> adw::ApplicationWindow {
    let live = Live {
        cpu: MetricCard::new(&tr("CPU"), Some(100.0)),
        memory: MetricCard::new(&tr("Memory"), Some(100.0)),
        disk: MetricCard::new(&tr("Disk"), None),
        network: MetricCard::new(&tr("Network"), None),
        processes: Rc::new(ProcessList::new()),
        devices: DevicesPage::new(),
    };
    let processes = live.processes.clone();
    let cards: Vec<gtk::Box> = [&live.cpu, &live.memory, &live.disk, &live.network]
        .map(|card| card.widget().clone())
        .into();
    let card_grid = gtk::Grid::builder()
        .column_spacing(12)
        .row_spacing(12)
        .column_homogeneous(true)
        .build();
    arrange(&card_grid, &cards, cards.len());

    let toasts = adw::ToastOverlay::new();
    let space = SpacePage::new();
    let startup = StartupPage::new(&toasts);
    let stack = adw::ViewStack::builder().vexpand(true).build();
    let table = gtk::Frame::builder().child(processes.widget()).build();
    stack.add_titled_with_icon(&table, Some(PROCESSES_PAGE), &tr("Processes"), "view-list-symbolic");
    stack.add_titled_with_icon(
        live.devices.widget(),
        Some("devices"),
        &tr("Devices"),
        "computer-symbolic",
    );
    let connections = Rc::new(ConnectionsPage::new());
    stack.add_titled_with_icon(
        connections.widget(),
        Some(CONNECTIONS_PAGE),
        &tr("Connections"),
        "network-transmit-receive-symbolic",
    );
    stack.add_titled_with_icon(
        space.widget(),
        Some("space"),
        &tr("Disk Space"),
        "drive-harddisk-symbolic",
    );
    stack.add_titled_with_icon(
        startup.widget(),
        Some(STARTUP_PAGE),
        &tr("Startup"),
        "system-run-symbolic",
    );
    follow_sockets(&stack, connections);
    // The entries are read each time the page comes into view, which also picks up changes made elsewhere.
    stack.connect_visible_child_name_notify(move |stack| {
        if stack.visible_child_name().as_deref() == Some(STARTUP_PAGE) {
            startup.refresh();
        }
    });
    // The page object owns the scan, so this handler keeps it alive for as long as the stack exists.
    stack.connect_destroy(move |_| {
        let _ = &space;
    });

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&card_grid);
    content.append(&stack);
    toasts.set_child(Some(&content));

    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text(tr("Search by name, PID, user or command"))
        .hexpand(true)
        .build();
    let search_bar = gtk::SearchBar::builder()
        .child(&adw::Clamp::builder().maximum_size(480).child(&search_entry).build())
        .build();
    search_bar.connect_entry(&search_entry);
    search_entry.connect_search_changed({
        let processes = processes.clone();
        move |entry| processes.set_search(&entry.text())
    });

    let search_button = gtk::ToggleButton::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text(tr("Search"))
        .build();
    search_button
        .bind_property("active", &search_bar, "search-mode-enabled")
        .bidirectional()
        .build();
    // An icon keeps the header from crowding the tabs in languages with longer words.
    let end_button = gtk::Button::builder()
        .icon_name("process-stop-symbolic")
        .tooltip_text(tr("End Process"))
        .action_name("win.end-process")
        .build();
    let menu = gio::Menu::new();
    menu.append(Some(&tr("Show as _Tree")), Some("win.show-process-tree"));
    menu.append(Some(&tr("Show _Kernel Threads")), Some("win.show-kernel-threads"));
    menu.append(Some(&tr("_About Pulse")), Some("app.about"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text(tr("Main Menu"))
        .primary(true)
        .menu_model(&menu)
        .build();

    let switcher = adw::ViewSwitcher::builder()
        .stack(&stack)
        .policy(adw::ViewSwitcherPolicy::Wide)
        .build();
    let header = adw::HeaderBar::builder().title_widget(&switcher).build();
    header.pack_start(&search_button);
    header.pack_end(&menu_button);
    header.pack_end(&end_button);
    let switcher_bar = adw::ViewSwitcherBar::builder().stack(&stack).build();
    let view = adw::ToolbarView::builder().content(&toasts).build();
    view.add_top_bar(&header);
    view.add_top_bar(&search_bar);
    view.add_bottom_bar(&switcher_bar);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Pulse")
        .default_width(1100)
        .default_height(760)
        .width_request(420)
        .height_request(560)
        .content(&view)
        .build();

    // A narrow window shows its title in the header, the tabs in a bar at the bottom and the cards two by two.
    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        NARROW_WIDTH,
        adw::LengthUnit::Sp,
    ));
    let title = adw::WindowTitle::new("Pulse", "");
    narrow.add_setter(&header, "title-widget", Some(&title.to_value()));
    narrow.add_setter(&switcher_bar, "reveal", Some(&true.to_value()));
    narrow.connect_apply({
        let (grid, cards) = (card_grid.clone(), cards.clone());
        move |_| arrange(&grid, &cards, 2)
    });
    narrow.connect_unapply(move |_| arrange(&card_grid, &cards, cards.len()));
    window.add_breakpoint(narrow);
    search_bar.set_key_capture_widget(Some(&window));

    // Search and End Process belong to the process table only.
    stack.connect_visible_child_name_notify({
        let (search_bar, search_button, end_button) = (search_bar.clone(), search_button.clone(), end_button.clone());
        let window = window.downgrade();
        move |stack| {
            let on_processes = stack.visible_child_name().as_deref() == Some(PROCESSES_PAGE);
            search_button.set_visible(on_processes);
            end_button.set_visible(on_processes);
            if !on_processes {
                search_bar.set_search_mode(false);
            }
            // Typing only opens the search while the table is in view.
            search_bar.set_key_capture_widget(window.upgrade().filter(|_| on_processes).as_ref());
        }
    });

    processes.connect_activated({
        let (window, toasts) = (window.downgrade(), toasts.clone());
        move |process| {
            if let Some(window) = window.upgrade() {
                present_process_details(&window, &toasts, &process);
            }
        }
    });
    let settings = settings::load();
    if let Some(settings) = &settings {
        settings.bind("window-width", &window, "default-width").build();
        settings.bind("window-height", &window, "default-height").build();
        settings.bind("window-maximized", &window, "maximized").build();
    }
    add_actions(&window, &processes, &search_bar, &stack, &toasts, settings.as_ref());
    start_sampling(&window, live);
    window
}

/// Lays the cards out in rows of `columns`.
fn arrange(grid: &gtk::Grid, cards: &[gtk::Box], columns: usize) {
    for card in cards {
        if card.parent().is_some() {
            grid.remove(card);
        }
    }
    for (index, card) in cards.iter().enumerate() {
        grid.attach(card, (index % columns) as i32, (index / columns) as i32, 1, 1);
    }
}

/// A menu item with a check mark. `name` is both the action and the settings key that remembers its state.
fn toggle(
    name: &'static str,
    settings: Option<&gio::Settings>,
    apply: impl Fn(bool) + 'static,
) -> gio::ActionEntry<adw::ApplicationWindow> {
    let initial = settings.is_some_and(|settings| settings.boolean(name));
    apply(initial);
    let settings = settings.cloned();
    gio::ActionEntry::builder(name)
        .state(initial.to_variant())
        .activate(move |_: &adw::ApplicationWindow, action, _| {
            let value = !action.state().and_then(|state| state.get::<bool>()).unwrap_or(false);
            action.set_state(&value.to_variant());
            apply(value);
            if let Some(settings) = &settings {
                // A read-only backend only costs the memory of the choice.
                let _ = settings.set_boolean(name, value);
            }
        })
        .build()
}

fn add_actions(
    window: &adw::ApplicationWindow,
    processes: &Rc<ProcessList>,
    search_bar: &gtk::SearchBar,
    stack: &adw::ViewStack,
    toasts: &adw::ToastOverlay,
    settings: Option<&gio::Settings>,
) {
    let search = gio::ActionEntry::builder("search")
        .activate({
            let (search_bar, stack) = (search_bar.clone(), stack.clone());
            move |_: &adw::ApplicationWindow, _, _| {
                stack.set_visible_child_name(PROCESSES_PAGE);
                search_bar.set_search_mode(!search_bar.is_search_mode());
            }
        })
        .build();
    let end_process = gio::ActionEntry::builder("end-process")
        .activate({
            let (processes, toasts) = (processes.clone(), toasts.clone());
            move |window: &adw::ApplicationWindow, _, _| {
                if let Some(process) = processes.selected() {
                    confirm_end(window, &toasts, &process);
                }
            }
        })
        .build();
    let show_kernel_threads = toggle("show-kernel-threads", settings, {
        let processes = processes.clone();
        move |show| processes.set_show_kernel_threads(show)
    });
    let show_process_tree = toggle("show-process-tree", settings, {
        let processes = processes.clone();
        move |tree| processes.set_tree(tree)
    });
    window.add_action_entries([search, end_process, show_kernel_threads, show_process_tree]);

    // Ending is only offered while a row is selected.
    let set_end_enabled = {
        let window = window.downgrade();
        move |enabled: bool| {
            let action = window.upgrade().and_then(|window| window.lookup_action("end-process"));
            if let Some(action) = action.and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
        }
    };
    set_end_enabled(false);
    processes.connect_selection_changed(move |selected| set_end_enabled(selected.is_some()));
}

/// Keeps the connections page current while it is the visible one, starting as soon as it is shown.
fn follow_sockets(stack: &adw::ViewStack, page: Rc<ConnectionsPage>) {
    let stack = stack.downgrade();
    glib::spawn_future_local(async move {
        let mut last: Option<Instant> = None;
        loop {
            let Some(visible) = stack.upgrade().map(|stack| stack.visible_child_name()) else {
                break;
            };
            if visible.as_deref() != Some(CONNECTIONS_PAGE) {
                last = None;
            } else if last.is_none_or(|last| last.elapsed() >= SOCKET_INTERVAL) {
                if let Ok(sockets) = gio::spawn_blocking(sockets::read).await {
                    page.update(&sockets);
                }
                last = Some(Instant::now());
            }
            glib::timeout_future(Duration::from_millis(250)).await;
        }
    });
}

/// Samples on a worker thread and applies each frame on the main loop.
/// The worker stops once the window, and with it the receiving end, is gone.
fn start_sampling(window: &adw::ApplicationWindow, live: Live) {
    let (sender, receiver) = async_channel::bounded::<Frame>(1);
    let worker = thread::Builder::new().name("sampler".into()).spawn(move || {
        let mut sampler = Sampler::default();
        while sender.send_blocking(sampler.sample()).is_ok() {
            thread::sleep(SAMPLE_INTERVAL);
        }
    });
    if let Err(error) = worker {
        glib::g_critical!("pulse", "Could not start the sampler thread: {error}");
        return;
    }

    let window = window.downgrade();
    glib::spawn_future_local(async move {
        while let Ok(frame) = receiver.recv().await {
            if window.upgrade().is_none() {
                break;
            }
            show(&live, &frame);
        }
    });
}

fn show(live: &Live, frame: &Frame) {
    let threads: u32 = frame.processes.iter().map(|process| process.threads).sum();
    live.cpu.update(
        frame.cpu,
        &format::percent(frame.cpu),
        &format!(
            "{} · {}",
            trn("{0} process", "{0} processes", frame.processes.len()),
            trn("{0} thread", "{0} threads", threads as usize)
        ),
    );

    let memory = (frame.memory_total > 0).then(|| 100.0 * frame.memory_used as f64 / frame.memory_total as f64);
    live.memory.update(
        memory,
        &format::percent(memory),
        &trf(
            "{0} of {1}",
            &[
                &format::bytes(frame.memory_used as f64),
                &format::bytes(frame.memory_total as f64),
            ],
        ),
    );

    let disk = frame.disk_read.zip(frame.disk_write).map(|(read, write)| read + write);
    live.disk.update(
        disk,
        &format::rate(disk),
        &trf(
            "Read {0} · Write {1}",
            &[&format::rate(frame.disk_read), &format::rate(frame.disk_write)],
        ),
    );

    let network = frame.received.zip(frame.sent).map(|(received, sent)| received + sent);
    live.network.update(
        network,
        &format::rate(network),
        &format!("↓ {} · ↑ {}", format::rate(frame.received), format::rate(frame.sent)),
    );

    live.processes.update(&frame.processes);
    live.devices.update(frame);
}

fn confirm_end(window: &adw::ApplicationWindow, toasts: &adw::ToastOverlay, process: &ProcessObject) {
    let (key, name) = (process.key(), process.name());
    let dialog = adw::AlertDialog::new(
        Some(&trf("End “{0}”?", &[&name])),
        Some(&trf(
            "Process {0} will be asked to exit. Force Stop ends it at once, without letting it save anything.",
            &[&key.pid],
        )),
    );
    dialog.add_responses(&[
        ("cancel", &tr("_Cancel")),
        ("kill", &tr("_Force Stop")),
        ("end", &tr("_End Process")),
    ]);
    dialog.set_response_appearance("kill", adw::ResponseAppearance::Destructive);
    dialog.set_response_appearance("end", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, {
        let toasts = toasts.clone();
        move |_, response| {
            if response == "cancel" {
                return;
            }
            if let Err(error) = telemetry::terminate(key, response == "kill") {
                toasts.add_toast(adw::Toast::new(&trf("Could not end “{0}”: {1}", &[&name, &error])));
            }
        }
    });
    dialog.present(Some(window));
}
