mod terminal;
mod window;

use libadwaita as adw;
use adw::prelude::*;

const APP_ID: &str = "dev.cmux.linux";

fn main() {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(window::build_window);

    // Register keyboard shortcuts
    app.set_accels_for_action("win.new-workspace", &["<Ctrl><Shift>n"]);
    app.set_accels_for_action("win.close-workspace", &["<Ctrl><Shift>w"]);
    app.set_accels_for_action("win.close-pane", &["<Ctrl>w"]);
    app.set_accels_for_action("win.toggle-sidebar", &["<Ctrl>b"]);
    app.set_accels_for_action("win.split-right", &["<Ctrl>d"]);
    app.set_accels_for_action("win.split-down", &["<Ctrl><Shift>d"]);
    app.set_accels_for_action("win.next-workspace", &["<Ctrl>Page_Down"]);
    app.set_accels_for_action("win.prev-workspace", &["<Ctrl>Page_Up"]);

    app.run();
}
