// SPDX-License-Identifier: GPL-3.0-or-later

mod format;
mod i18n;
mod telemetry;
mod ui;
mod window;

use adw::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "io.github.makarov_mm.Pulse";

fn main() -> glib::ExitCode {
    i18n::init();
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(setup_actions);
    app.connect_activate(|app| {
        // A second launch raises the window that is already open.
        let window = app.active_window().unwrap_or_else(|| window::build(app).upcast());
        window.present();
    });
    app.run()
}

fn setup_actions(app: &adw::Application) {
    let quit = gio::ActionEntry::builder("quit")
        .activate(|app: &adw::Application, _, _| app.quit())
        .build();
    let about = gio::ActionEntry::builder("about")
        .activate(|app: &adw::Application, _, _| show_about(app))
        .build();
    app.add_action_entries([quit, about]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
    app.set_accels_for_action("win.search", &["<Control>f"]);
}

fn show_about(app: &adw::Application) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Pulse")
        .application_icon(APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("Mykhailo Makarov")
        .website(env!("CARGO_PKG_REPOSITORY"))
        .license_type(gtk::License::Gpl30)
        // Translators: put your name here, one translator per line.
        .translator_credits(i18n::tr("translator-credits"))
        .build();
    dialog.present(app.active_window().as_ref());
}
