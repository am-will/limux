use std::env;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

const LIMUX_SOCKET_ENV: &str = "LIMUX_SOCKET";
const LIMUX_SOCKET_PATH_ENV: &str = "LIMUX_SOCKET_PATH";
const RUNTIME_SUBDIR: &str = "limux";
const RUNTIME_SOCKET_NAME: &str = "limux.sock";
const FALLBACK_RUNTIME_SOCKET: &str = "/tmp/limux.sock";
const DEBUG_SOCKET: &str = "/tmp/limux-debug.sock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketMode {
    Runtime,
    Debug,
}

impl SocketMode {
    pub fn default_for(mode: Self) -> PathBuf {
        match mode {
            Self::Runtime => default_runtime_socket_path(),
            Self::Debug => PathBuf::from(DEBUG_SOCKET),
        }
    }
}

pub fn resolve_socket_path(explicit: Option<PathBuf>, mode: SocketMode) -> PathBuf {
    if let Some(path) = explicit {
        return path;
    }

    if let Some(path) = get_env_path(LIMUX_SOCKET_ENV) {
        return path;
    }
    if let Some(path) = get_env_path(LIMUX_SOCKET_PATH_ENV) {
        return path;
    }

    SocketMode::default_for(mode)
}

fn default_runtime_socket_path() -> PathBuf {
    match env::var_os("XDG_RUNTIME_DIR") {
        Some(runtime_dir) if !runtime_dir.is_empty() => {
            let mut path = PathBuf::from(runtime_dir);
            if runtime_dir_is_secure(&path) {
                path.push(RUNTIME_SUBDIR);
                path.push(RUNTIME_SOCKET_NAME);
                path
            } else {
                PathBuf::from(FALLBACK_RUNTIME_SOCKET)
            }
        }
        _ => PathBuf::from(FALLBACK_RUNTIME_SOCKET),
    }
}

fn runtime_dir_is_secure(path: &std::path::Path) -> bool {
    if !path.is_absolute() {
        return false;
    }

    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };

    metadata.is_dir()
        && metadata.uid() == unsafe { libc::getuid() }
        && (metadata.mode() & 0o777) == 0o700
}

fn get_env_path(key: &str) -> Option<PathBuf> {
    env::var_os(key).and_then(|value| {
        if value.is_empty() {
            None
        } else {
            Some(PathBuf::from(value))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;
    use tempfile::TempDir;

    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let old = env::var_os(key);
            match value {
                Some(value) => unsafe { env::set_var(key, value) },
                None => unsafe { env::remove_var(key) },
            }
            Self { key, old }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old {
                Some(value) => unsafe { env::set_var(self.key, value) },
                None => unsafe { env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn explicit_path_has_highest_precedence() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let _socket = EnvGuard::set(LIMUX_SOCKET_ENV, Some("/tmp/from-env.sock"));
        let _socket_path = EnvGuard::set(LIMUX_SOCKET_PATH_ENV, Some("/tmp/from-env-path.sock"));

        let resolved = resolve_socket_path(
            Some(PathBuf::from("/tmp/from-arg.sock")),
            SocketMode::Runtime,
        );
        assert_eq!(resolved, PathBuf::from("/tmp/from-arg.sock"));
    }

    #[test]
    fn limux_socket_has_higher_precedence_than_limux_socket_path() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let _socket = EnvGuard::set(LIMUX_SOCKET_ENV, Some("/tmp/from-limux-socket.sock"));
        let _socket_path = EnvGuard::set(
            LIMUX_SOCKET_PATH_ENV,
            Some("/tmp/from-limux-socket-path.sock"),
        );

        let resolved = resolve_socket_path(None, SocketMode::Runtime);
        assert_eq!(resolved, PathBuf::from("/tmp/from-limux-socket.sock"));
    }

    #[test]
    fn limux_socket_path_used_when_limux_socket_missing() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let _socket = EnvGuard::set(LIMUX_SOCKET_ENV, None);
        let _socket_path = EnvGuard::set(
            LIMUX_SOCKET_PATH_ENV,
            Some("/tmp/from-limux-socket-path.sock"),
        );

        let resolved = resolve_socket_path(None, SocketMode::Runtime);
        assert_eq!(resolved, PathBuf::from("/tmp/from-limux-socket-path.sock"));
    }

    #[test]
    fn runtime_mode_defaults_to_xdg_runtime_dir() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let _socket = EnvGuard::set(LIMUX_SOCKET_ENV, None);
        let _socket_path = EnvGuard::set(LIMUX_SOCKET_PATH_ENV, None);
        let xdg = TempDir::new().expect("xdg runtime dir temp path");
        std::fs::set_permissions(
            xdg.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .expect("set runtime dir perms");
        let _xdg = EnvGuard::set("XDG_RUNTIME_DIR", Some(xdg.path().to_str().expect("utf8")));

        let resolved = resolve_socket_path(None, SocketMode::Runtime);
        assert_eq!(
            resolved,
            xdg.path().join(RUNTIME_SUBDIR).join(RUNTIME_SOCKET_NAME)
        );
    }

    #[test]
    fn debug_mode_defaults_to_debug_socket() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let _socket = EnvGuard::set(LIMUX_SOCKET_ENV, None);
        let _socket_path = EnvGuard::set(LIMUX_SOCKET_PATH_ENV, None);
        let _xdg = EnvGuard::set("XDG_RUNTIME_DIR", None);

        let resolved = resolve_socket_path(None, SocketMode::Debug);
        assert_eq!(resolved, PathBuf::from(DEBUG_SOCKET));
    }

    #[test]
    fn insecure_runtime_dir_falls_back_to_tmp_socket() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let _socket = EnvGuard::set(LIMUX_SOCKET_ENV, None);
        let _socket_path = EnvGuard::set(LIMUX_SOCKET_PATH_ENV, None);
        let xdg = TempDir::new().expect("xdg runtime dir temp path");
        std::fs::set_permissions(
            xdg.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .expect("set runtime dir perms");
        let _xdg = EnvGuard::set("XDG_RUNTIME_DIR", Some(xdg.path().to_str().expect("utf8")));

        let resolved = resolve_socket_path(None, SocketMode::Runtime);
        assert_eq!(resolved, PathBuf::from(FALLBACK_RUNTIME_SOCKET));
    }
}
