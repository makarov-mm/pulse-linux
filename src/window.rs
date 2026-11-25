// SPDX-License-Identifier: GPL-3.0-or-later

use std::rc::Rc;
use std::thread;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::format;
use crate::telemetry::{self, Frame, Sampler};
use crate::ui::{DevicesPage, MetricCard, ProcessList, ProcessObject, SpacePage};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const PROCESSES_PAGE: &str = "processes";

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
        cpu: MetricCard::new("CPU", Some(100.0)),
        memory: MetricCard::new("Memory", Some(100.0)),
        disk: MetricCard::new("Disk", None),
        network: MetricCard::new("Network", None),
        processes: Rc::new(ProcessList::new()),
        devices: DevicesPage::new(),
    };
    let processes = live.processes.clone();
    let card_row = gtk::Box::builder().spacing(12).homogeneous(true).build();
    for card in [&live.cpu, &live.memory, &live.disk, &live.network] {
        card_row.append(card.widget());
    }

    let space = SpacePage::new();
    let stack = adw::ViewStack::builder().vexpand(true).build();
    let table = gtk::Frame::builder().child(processes.widget()).build();
    stack.add_titled_with_icon(&table, Some(PROCESSES_PAGE), "Processes", "view-list-symbolic");
    stack.add_titled_with_icon(live.devices.widget(), Some("devices"), "Devices", "computer-symbolic");
    stack.add_titled_with_icon(space.widget(), Some("space"), "Disk Space", "drive-harddisk-symbolic");
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
    content.append(&card_row);
    content.append(&stack);
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&content));

    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text("Search by name, PID, user or command")
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
        .tooltip_text("Search")
        .build();
    search_button
        .bind_property("active", &search_bar, "search-mode-enabled")
        .bidirectional()
        .build();
    let end_button = gtk::Button::builder()
        .label("End Process")
        .action_name("win.end-process")
        .build();
    let menu = gio::Menu::new();
    menu.append(Some("Show _Kernel Threads"), Some("win.show-kernel-threads"));
    menu.append(Some("_About Pulse"), Some("app.about"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main Menu")
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
    let view = adw::ToolbarView::builder().content(&toasts).build();
    view.add_top_bar(&header);
    view.add_top_bar(&search_bar);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Pulse")
        .default_width(1100)
        .default_height(760)
        .content(&view)
        .build();
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

    add_actions(&window, &processes, &search_bar, &stack, &toasts);
    start_sampling(&window, live);
    window
}

fn add_actions(
    window: &adw::ApplicationWindow,
    processes: &Rc<ProcessList>,
    search_bar: &gtk::SearchBar,
    stack: &adw::ViewStack,
    toasts: &adw::ToastOverlay,
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
    let show_kernel_threads = gio::ActionEntry::builder("show-kernel-threads")
        .state(false.to_variant())
        .activate({
            let processes = processes.clone();
            move |_: &adw::ApplicationWindow, action, _| {
                let show = !action.state().and_then(|state| state.get::<bool>()).unwrap_or(false);
                action.set_state(&show.to_variant());
                processes.set_show_kernel_threads(show);
            }
        })
        .build();
    window.add_action_entries([search, end_process, show_kernel_threads]);

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
        &format!("{} processes · {threads} threads", frame.processes.len()),
    );

    let memory = (frame.memory_total > 0).then(|| 100.0 * frame.memory_used as f64 / frame.memory_total as f64);
    live.memory.update(
        memory,
        &format::percent(memory),
        &format!(
            "{} of {}",
            format::bytes(frame.memory_used as f64),
            format::bytes(frame.memory_total as f64)
        ),
    );

    let disk = frame.disk_read.zip(frame.disk_write).map(|(read, write)| read + write);
    live.disk.update(
        disk,
        &format::rate(disk),
        &format!(
            "Read {} · Write {}",
            format::rate(frame.disk_read),
            format::rate(frame.disk_write)
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
        Some(&format!("End “{name}”?")),
        Some(&format!(
            "Process {} will be asked to exit. Force Stop ends it at once, without letting it save anything.",
            key.pid
        )),
    );
    dialog.add_responses(&[("cancel", "_Cancel"), ("kill", "_Force Stop"), ("end", "_End Process")]);
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
                toasts.add_toast(adw::Toast::new(&format!("Could not end “{name}”: {error}")));
            }
        }
    });
    dialog.present(Some(window));
}
