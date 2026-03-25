mod keybind_editor;
mod layout_state;
mod pane;
mod shortcut_config;
mod terminal;
mod window;

use adw::prelude::*;
use libadwaita as adw;
use std::path::{Path, PathBuf};

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

fn is_ghostty_resources_dir(path: &Path) -> bool {
    path.is_dir()
        && ["themes", "terminfo", "shell-integration"]
            .iter()
            .any(|entry| path.join(entry).is_dir())
}

fn ghostty_resources_candidates(exe_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    for ancestor in exe_dir.ancestors() {
        candidates.push(ancestor.join("share/limux/ghostty"));
        candidates.push(ancestor.join("share/ghostty"));
        candidates.push(ancestor.join("ghostty/zig-out/share/ghostty"));
    }

    candidates.push(PathBuf::from("/usr/local/share/ghostty"));
    candidates.push(PathBuf::from("/usr/share/ghostty"));

    candidates
}

fn resolve_ghostty_resources_dir(exe_path: &Path) -> Option<PathBuf> {
    let exe_dir = exe_path.parent()?;
    ghostty_resources_candidates(exe_dir)
        .into_iter()
        .find(|path| is_ghostty_resources_dir(path))
}

fn set_ghostty_resources_env() {
    if std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some() {
        return;
    }

    let Some(exe_path) = std::env::current_exe().ok() else {
        return;
    };

    if let Some(path) = resolve_ghostty_resources_dir(&exe_path) {
        std::env::set_var("GHOSTTY_RESOURCES_DIR", path);
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

    // Embedded Ghostty needs a resources directory to resolve named themes,
    // terminfo, and shell integration. Prefer Limux-bundled resources but
    // fall back to common system Ghostty install locations.
    set_ghostty_resources_env();

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

    app.connect_activate(move |app| {
        window::build_window(app);
    });
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        std::env::temp_dir().join(format!("limux-{label}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn resolves_app_specific_bundled_resources_next_to_executable() {
        let root = temp_path("resources");
        let exe_dir = root.join("bin");
        let resources_dir = root.join("share/limux/ghostty/themes");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::create_dir_all(&resources_dir).unwrap();

        let exe = exe_dir.join("limux");
        let resolved = resolve_ghostty_resources_dir(&exe).unwrap();
        assert_eq!(resolved, root.join("share/limux/ghostty"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_dev_checkout_resources_from_target_binary() {
        let root = temp_path("dev-resources");
        let exe_dir = root.join("target/release");
        let resources_dir = root.join("ghostty/zig-out/share/ghostty/terminfo");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::create_dir_all(&resources_dir).unwrap();

        let exe = exe_dir.join("limux");
        let resolved = resolve_ghostty_resources_dir(&exe).unwrap();
        assert_eq!(resolved, root.join("ghostty/zig-out/share/ghostty"));

        fs::remove_dir_all(root).unwrap();
    }
}
