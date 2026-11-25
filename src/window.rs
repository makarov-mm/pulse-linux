// SPDX-License-Identifier: GPL-3.0-or-later

use std::rc::Rc;
use std::thread;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::format;
use crate::telemetry::{self, Frame, Sampler};
use crate::ui::{MetricCard, ProcessList, ProcessObject};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

struct Cards {
    cpu: MetricCard,
    memory: MetricCard,
    disk: MetricCard,
    network: MetricCard,
}

pub fn build(app: &adw::Application) -> adw::ApplicationWindow {
    let cards = Cards {
        cpu: MetricCard::new("CPU", Some(100.0)),
        memory: MetricCard::new("Memory", Some(100.0)),
        disk: MetricCard::new("Disk", None),
        network: MetricCard::new("Network", None),
    };
    let card_row = gtk::Box::builder().spacing(12).homogeneous(true).build();
    for card in [&cards.cpu, &cards.memory, &cards.disk, &cards.network] {
        card_row.append(card.widget());
    }

    let processes = Rc::new(ProcessList::new());
    let table = gtk::Frame::builder().child(processes.widget()).build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&card_row);
    content.append(&table);
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&content));

    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text("Search processes")
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
    menu.append(Some("_About Pulse"), Some("app.about"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main Menu")
        .primary(true)
        .menu_model(&menu)
        .build();

    let header = adw::HeaderBar::new();
    header.pack_start(&search_button);
    header.pack_end(&menu_button);
    header.pack_end(&end_button);
    let view = adw::ToolbarView::builder().content(&toasts).build();
    view.add_top_bar(&header);
    view.add_top_bar(&search_bar);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Pulse")
        .default_width(1000)
        .default_height(720)
        .content(&view)
        .build();
    search_bar.set_key_capture_widget(Some(&window));

    let search = gio::ActionEntry::builder("search")
        .activate({
            let search_bar = search_bar.clone();
            move |_: &adw::ApplicationWindow, _, _| search_bar.set_search_mode(!search_bar.is_search_mode())
        })
        .build();
    let end_process = gio::ActionEntry::builder("end-process")
        .activate({
            let processes = processes.clone();
            let toasts = toasts.clone();
            move |window: &adw::ApplicationWindow, _, _| {
                if let Some(process) = processes.selected() {
                    confirm_end(window, &toasts, &process);
                }
            }
        })
        .build();
    window.add_action_entries([search, end_process]);

    // The action is only offered while a row is selected.
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

    start_sampling(&window, cards, processes);
    window
}

/// Samples on a worker thread and applies each frame on the main loop.
/// The worker stops once the window, and with it the receiving end, is gone.
fn start_sampling(window: &adw::ApplicationWindow, cards: Cards, processes: Rc<ProcessList>) {
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
            show(&cards, &frame);
            processes.update(&frame.processes);
        }
    });
}

fn show(cards: &Cards, frame: &Frame) {
    cards.cpu.update(
        frame.cpu,
        &format::percent(frame.cpu),
        &format!("{} processes", frame.processes.len()),
    );

    let memory = (frame.memory_total > 0).then(|| 100.0 * frame.memory_used as f64 / frame.memory_total as f64);
    cards.memory.update(
        memory,
        &format::percent(memory),
        &format!(
            "{} of {}",
            format::bytes(frame.memory_used as f64),
            format::bytes(frame.memory_total as f64)
        ),
    );

    let disk = frame.disk_read.zip(frame.disk_write).map(|(read, write)| read + write);
    cards.disk.update(
        disk,
        &format::rate(disk),
        &format!("Read {} · Write {}", format::rate(frame.disk_read), format::rate(frame.disk_write)),
    );

    let network = frame.received.zip(frame.sent).map(|(received, sent)| received + sent);
    cards.network.update(
        network,
        &format::rate(network),
        &format!("↓ {} · ↑ {}", format::rate(frame.received), format::rate(frame.sent)),
    );
}

fn confirm_end(window: &adw::ApplicationWindow, toasts: &adw::ToastOverlay, process: &ProcessObject) {
    let (key, name) = (process.key(), process.name());
    let dialog = adw::AlertDialog::new(
        Some(&format!("End “{name}”?")),
        Some(&format!("Process {} will be asked to exit. Unsaved data may be lost.", key.pid)),
    );
    dialog.add_responses(&[("cancel", "_Cancel"), ("end", "_End Process")]);
    dialog.set_response_appearance("end", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(Some("end"), {
        let toasts = toasts.clone();
        move |_, _| {
            if let Err(error) = telemetry::terminate(key) {
                toasts.add_toast(adw::Toast::new(&format!("Could not end “{name}”: {error}")));
            }
        }
    });
    dialog.present(Some(window));
}
