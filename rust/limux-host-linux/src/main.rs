mod layout_state;
mod pane;
mod terminal;
mod window;

use adw::prelude::*;
use libadwaita as adw;

const APP_ID: &str = "dev.limux.linux";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Append a value to an environment variable (comma-separated), or set it.
fn append_env(key: &str, value: &str) {
    match std::env::var(key) {
        Ok(existing) if !existing.is_empty() => {
            std::env::set_var(key, format!("{existing},{value}"));
        }
        _ => {
            std::env::set_var(key, value);
        }
    }
}

fn main() {
    // Handle --version flag
    if std::env::args().any(|a| a == "--version" || a == "-v") {
        println!("Limux {VERSION}");
        return;
    }

    // Ghostty requires desktop OpenGL, not GLES. Must disable GLES before
    // GTK initializes, otherwise GDK may select a GLES context.
    // This matches what Ghostty's own GTK apprt does in setGtkEnv().
    append_env("GDK_DISABLE", "gles-api,vulkan");
    append_env("GDK_DEBUG", "gl-disable-gles,vulkan-disable");

    // WebKitGTK's bubblewrap sandbox requires unprivileged user namespaces,
    // which may not be available. Disable it to prevent crashes on launch.
    if std::env::var("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS").is_err() {
        std::env::set_var("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS", "1");
    }

    // Initialize Ghostty before GTK app starts
    terminal::init_ghostty();

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(adw::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(window::build_window);

    // Global keyboard shortcuts
    app.set_accels_for_action("win.new-workspace", &["<Ctrl><Shift>n"]);
    app.set_accels_for_action("win.close-workspace", &["<Ctrl><Shift>w"]);
    app.set_accels_for_action("win.toggle-sidebar", &["<Ctrl>b"]);
    app.set_accels_for_action("win.next-workspace", &["<Ctrl>Page_Down"]);
    app.set_accels_for_action("win.prev-workspace", &["<Ctrl>Page_Up"]);

    app.run();
}
