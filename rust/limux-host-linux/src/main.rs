mod app_config;
mod control_bridge;
mod ghostty_config;
mod keybind_editor;
mod layout_state;
mod pane;
mod settings_editor;
mod shortcut_config;
mod split_tree;
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

fn has_ghostty_terminfo(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };

    ["terminfo/g/ghostty", "terminfo/x/xterm-ghostty"]
        .iter()
        .any(|entry| parent.join(entry).is_file())
}

fn is_ghostty_resources_dir(path: &Path) -> bool {
    path.is_dir()
        && ["themes", "shell-integration"]
            .iter()
            .all(|entry| path.join(entry).is_dir())
        && has_ghostty_terminfo(path)
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

fn ghostty_terminfo_dir(resources_dir: &Path) -> Option<PathBuf> {
    resources_dir.parent().map(|parent| parent.join("terminfo"))
}

fn set_env_path_if_missing_or_invalid(
    key: &str,
    path: Option<PathBuf>,
    validator: impl Fn(&Path) -> bool,
) {
    let has_valid_existing = std::env::var_os(key)
        .map(PathBuf::from)
        .is_some_and(|existing| validator(&existing));

    if has_valid_existing {
        return;
    }

    if let Some(path) = path.filter(|candidate| validator(candidate)) {
        std::env::set_var(key, path);
    }
}

fn set_ghostty_runtime_env_for_exe(exe_path: &Path) {
    let Some(resources_dir) = resolve_ghostty_resources_dir(exe_path) else {
        return;
    };

    set_env_path_if_missing_or_invalid(
        "GHOSTTY_RESOURCES_DIR",
        Some(resources_dir.clone()),
        is_ghostty_resources_dir,
    );
    set_env_path_if_missing_or_invalid(
        "TERMINFO",
        ghostty_terminfo_dir(&resources_dir),
        has_ghostty_terminfo,
    );
    set_env_path_if_missing_or_invalid(
        "GHOSTTY_SHELL_INTEGRATION_XDG_DIR",
        Some(resources_dir.join("shell-integration")),
        |candidate| candidate.is_dir(),
    );
}

fn set_ghostty_runtime_env() {
    let Some(exe_path) = std::env::current_exe().ok() else {
        return;
    };

    set_ghostty_runtime_env_for_exe(&exe_path);
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
    set_ghostty_runtime_env();

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
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static GHOSTTY_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct GhosttyEnvGuard {
        resources: Option<std::ffi::OsString>,
        terminfo: Option<std::ffi::OsString>,
        shell_integration: Option<std::ffi::OsString>,
    }

    impl GhosttyEnvGuard {
        fn capture() -> Self {
            Self {
                resources: std::env::var_os("GHOSTTY_RESOURCES_DIR"),
                terminfo: std::env::var_os("TERMINFO"),
                shell_integration: std::env::var_os("GHOSTTY_SHELL_INTEGRATION_XDG_DIR"),
            }
        }
    }

    impl Drop for GhosttyEnvGuard {
        fn drop(&mut self) {
            match self.resources.take() {
                Some(value) => std::env::set_var("GHOSTTY_RESOURCES_DIR", value),
                None => std::env::remove_var("GHOSTTY_RESOURCES_DIR"),
            }
            match self.terminfo.take() {
                Some(value) => std::env::set_var("TERMINFO", value),
                None => std::env::remove_var("TERMINFO"),
            }
            match self.shell_integration.take() {
                Some(value) => std::env::set_var("GHOSTTY_SHELL_INTEGRATION_XDG_DIR", value),
                None => std::env::remove_var("GHOSTTY_SHELL_INTEGRATION_XDG_DIR"),
            }
        }
    }

    fn with_ghostty_env<R>(test: impl FnOnce() -> R) -> R {
        let _lock = GHOSTTY_ENV_LOCK
            .lock()
            .expect("ghostty env test lock poisoned");
        let _guard = GhosttyEnvGuard::capture();
        test()
    }

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
        let themes_dir = root.join("share/limux/ghostty/themes");
        let shell_integration_dir = root.join("share/limux/ghostty/shell-integration");
        let terminfo_file = root.join("share/limux/terminfo/g/ghostty");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::create_dir_all(&themes_dir).unwrap();
        fs::create_dir_all(&shell_integration_dir).unwrap();
        fs::create_dir_all(terminfo_file.parent().unwrap()).unwrap();
        fs::write(&terminfo_file, b"ghostty").unwrap();

        let exe = exe_dir.join("limux");
        let resolved = resolve_ghostty_resources_dir(&exe).unwrap();
        assert_eq!(resolved, root.join("share/limux/ghostty"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_dev_checkout_resources_from_target_binary() {
        let root = temp_path("dev-resources");
        let exe_dir = root.join("target/release");
        let themes_dir = root.join("ghostty/zig-out/share/ghostty/themes");
        let shell_integration_dir = root.join("ghostty/zig-out/share/ghostty/shell-integration");
        let terminfo_file = root.join("ghostty/zig-out/share/terminfo/x/xterm-ghostty");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::create_dir_all(&themes_dir).unwrap();
        fs::create_dir_all(&shell_integration_dir).unwrap();
        fs::create_dir_all(terminfo_file.parent().unwrap()).unwrap();
        fs::write(&terminfo_file, b"xterm-ghostty").unwrap();

        let exe = exe_dir.join("limux");
        let resolved = resolve_ghostty_resources_dir(&exe).unwrap();
        assert_eq!(resolved, root.join("ghostty/zig-out/share/ghostty"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_resource_dirs_without_sibling_terminfo() {
        let root = temp_path("missing-terminfo");
        let resources_dir = root.join("ghostty/zig-out/share/ghostty");
        let themes_dir = resources_dir.join("themes");
        let shell_integration_dir = resources_dir.join("shell-integration");
        fs::create_dir_all(&themes_dir).unwrap();
        fs::create_dir_all(&shell_integration_dir).unwrap();

        assert!(!is_ghostty_resources_dir(&resources_dir));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn derives_terminfo_dir_from_resources_dir() {
        let resources_dir = PathBuf::from("/usr/share/limux/ghostty");
        assert_eq!(
            ghostty_terminfo_dir(&resources_dir),
            Some(PathBuf::from("/usr/share/limux/terminfo"))
        );
    }

    #[test]
    fn replaces_invalid_inherited_runtime_env_with_resolved_paths() {
        with_ghostty_env(|| {
            let root = temp_path("env-override");
            let exe_dir = root.join("target/release");
            let resources_dir = root.join("ghostty/zig-out/share/ghostty");
            let themes_dir = resources_dir.join("themes");
            let shell_integration_dir = resources_dir.join("shell-integration");
            let terminfo_dir = root.join("ghostty/zig-out/share/terminfo");
            let terminfo_file = terminfo_dir.join("x/xterm-ghostty");
            fs::create_dir_all(&exe_dir).unwrap();
            fs::create_dir_all(&themes_dir).unwrap();
            fs::create_dir_all(&shell_integration_dir).unwrap();
            fs::create_dir_all(terminfo_file.parent().unwrap()).unwrap();
            fs::write(&terminfo_file, b"xterm-ghostty").unwrap();

            std::env::set_var("GHOSTTY_RESOURCES_DIR", "/app/share/limux/ghostty");
            std::env::set_var("TERMINFO", "/app/share/limux/terminfo");
            std::env::set_var(
                "GHOSTTY_SHELL_INTEGRATION_XDG_DIR",
                "/app/share/limux/ghostty/shell-integration",
            );

            let exe = exe_dir.join("limux");
            set_ghostty_runtime_env_for_exe(&exe);

            assert_eq!(
                std::env::var_os("GHOSTTY_RESOURCES_DIR"),
                Some(resources_dir.into_os_string())
            );
            assert_eq!(
                std::env::var_os("TERMINFO"),
                Some(terminfo_dir.into_os_string())
            );
            assert_eq!(
                std::env::var_os("GHOSTTY_SHELL_INTEGRATION_XDG_DIR"),
                Some(shell_integration_dir.into_os_string())
            );

            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn preserves_valid_existing_runtime_env_paths() {
        with_ghostty_env(|| {
            let root = temp_path("env-preserve");
            let exe_dir = root.join("target/release");
            let resources_dir = root.join("ghostty/zig-out/share/ghostty");
            let themes_dir = resources_dir.join("themes");
            let shell_integration_dir = resources_dir.join("shell-integration");
            let terminfo_dir = root.join("ghostty/zig-out/share/terminfo");
            let terminfo_file = terminfo_dir.join("x/xterm-ghostty");
            fs::create_dir_all(&exe_dir).unwrap();
            fs::create_dir_all(&themes_dir).unwrap();
            fs::create_dir_all(&shell_integration_dir).unwrap();
            fs::create_dir_all(terminfo_file.parent().unwrap()).unwrap();
            fs::write(&terminfo_file, b"xterm-ghostty").unwrap();

            std::env::set_var("GHOSTTY_RESOURCES_DIR", &resources_dir);
            std::env::set_var("TERMINFO", &terminfo_dir);
            std::env::set_var("GHOSTTY_SHELL_INTEGRATION_XDG_DIR", &shell_integration_dir);

            let exe = exe_dir.join("limux");
            set_ghostty_runtime_env_for_exe(&exe);

            assert_eq!(
                std::env::var_os("GHOSTTY_RESOURCES_DIR"),
                Some(resources_dir.into_os_string())
            );
            assert_eq!(
                std::env::var_os("TERMINFO"),
                Some(terminfo_dir.into_os_string())
            );
            assert_eq!(
                std::env::var_os("GHOSTTY_SHELL_INTEGRATION_XDG_DIR"),
                Some(shell_integration_dir.into_os_string())
            );

            fs::remove_dir_all(root).unwrap();
        });
    }
}
