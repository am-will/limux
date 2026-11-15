use std::env;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener as StdUnixListener, UnixStream as StdUnixStream};
use std::path::Path;
use std::path::PathBuf;

const LIMUX_SOCKET_ENV: &str = "LIMUX_SOCKET";
const LIMUX_SOCKET_PATH_ENV: &str = "LIMUX_SOCKET_PATH";
const RUNTIME_SUBDIR: &str = "limux";
const RUNTIME_SOCKET_NAME: &str = "limux.sock";
const DEBUG_SOCKET_NAME: &str = "limux-debug.sock";
const PRIVATE_DIR_MODE: u32 = 0o700;
const SOCKET_FILE_MODE: u32 = 0o600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketMode {
    Runtime,
    Debug,
}

impl SocketMode {
    pub fn default_for(mode: Self) -> PathBuf {
        let dir = default_runtime_dir();
        match mode {
            Self::Runtime => dir.join(RUNTIME_SOCKET_NAME),
            Self::Debug => dir.join(DEBUG_SOCKET_NAME),
        }
    }
}

pub fn current_euid() -> u32 {
    unsafe { libc::geteuid() }
}

pub fn default_runtime_dir() -> PathBuf {
    if let Some(runtime_dir) = env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        let candidate = PathBuf::from(&runtime_dir);
        if is_usable_xdg_runtime_dir(&candidate, current_euid()) {
            return candidate.join(RUNTIME_SUBDIR);
        }
    }
    env::temp_dir().join(format!("limux-{}", current_euid()))
}

fn is_usable_xdg_runtime_dir(candidate: &Path, expected_uid: u32) -> bool {
    if !candidate.is_absolute() {
        return false;
    }
    match fs::symlink_metadata(candidate) {
        Ok(meta) => {
            !meta.file_type().is_symlink()
                && meta.file_type().is_dir()
                && meta.uid() == expected_uid
                && (meta.permissions().mode() & 0o077) == 0
        }
        Err(_) => false,
    }
}

pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    ensure_private_dir_for_uid(path, current_euid())
}

pub fn ensure_private_dir_for_uid(path: &Path, expected_uid: u32) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_and_chmod_private_dir(path, &metadata, expected_uid)?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(PRIVATE_DIR_MODE)
                .create(path)?;
            let metadata = fs::symlink_metadata(path)?;
            validate_and_chmod_private_dir(path, &metadata, expected_uid)?;
        }
        Err(err) => return Err(err),
    }
    Ok(())
}

fn validate_and_chmod_private_dir(
    path: &Path,
    metadata: &fs::Metadata,
    expected_uid: u32,
) -> io::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing non-directory or symlink runtime path {}",
                path.display()
            ),
        ));
    }
    if metadata.uid() != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "refusing runtime directory {} owned by uid={} (expected uid={expected_uid})",
                path.display(),
                metadata.uid()
            ),
        ));
    }
    if (metadata.permissions().mode() & 0o777) != PRIVATE_DIR_MODE {
        fs::set_permissions(path, PermissionsExt::from_mode(PRIVATE_DIR_MODE))?;
        let refreshed = fs::symlink_metadata(path)?;
        if refreshed.file_type().is_symlink()
            || !refreshed.file_type().is_dir()
            || refreshed.uid() != expected_uid
            || (refreshed.permissions().mode() & 0o777) != PRIVATE_DIR_MODE
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "runtime directory {} permissions are not private (0700)",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

pub fn ensure_private_runtime_dir() -> io::Result<PathBuf> {
    let dir = default_runtime_dir();
    ensure_private_dir(&dir)?;
    Ok(dir)
}

pub fn ensure_private_runtime_subdir(name: &str) -> io::Result<PathBuf> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid runtime subdirectory name",
        ));
    }
    let dir = ensure_private_runtime_dir()?.join(name);
    ensure_private_dir(&dir)?;
    Ok(dir)
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

pub fn prepare_socket_path(path: &Path, mode: SocketMode, owner_only: bool) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if owner_only && should_lock_down_parent(path, mode) {
            ensure_private_dir(parent)?;
        } else {
            fs::create_dir_all(parent)?;
        }
    }
    remove_existing_socket(path)?;
    Ok(())
}

pub fn finalize_socket_permissions(path: &Path, owner_only: bool) -> io::Result<()> {
    if owner_only {
        fs::set_permissions(path, PermissionsExt::from_mode(SOCKET_FILE_MODE))?;
    }
    Ok(())
}

pub fn bind_listener(
    path: &Path,
    mode: SocketMode,
    owner_only: bool,
) -> io::Result<StdUnixListener> {
    prepare_socket_path(path, mode, owner_only)?;
    let listener = StdUnixListener::bind(path)?;
    finalize_socket_permissions(path, owner_only)?;
    Ok(listener)
}

pub fn bind_tokio_listener(
    path: &Path,
    mode: SocketMode,
    owner_only: bool,
) -> io::Result<tokio::net::UnixListener> {
    prepare_socket_path(path, mode, owner_only)?;
    let listener = tokio::net::UnixListener::bind(path)?;
    finalize_socket_permissions(path, owner_only)?;
    Ok(listener)
}

fn should_lock_down_parent(path: &Path, _mode: SocketMode) -> bool {
    path.parent() == Some(default_runtime_dir().as_path())
}

fn remove_existing_socket(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    if !metadata.file_type().is_socket() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("refusing to overwrite non-socket path {}", path.display()),
        ));
    }

    match StdUnixStream::connect(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("socket already in use at {}", path.display()),
        )),
        Err(error) if is_stale_socket_error(&error) => {
            fs::remove_file(path)?;
            Ok(())
        }
        Err(error) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("refusing to replace socket at {}: {error}", path.display()),
        )),
    }
}

fn is_stale_socket_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
    )
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

    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
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
        std::fs::set_permissions(xdg.path(), PermissionsExt::from_mode(0o700))
            .expect("chmod xdg 0700");
        let _xdg = EnvGuard::set("XDG_RUNTIME_DIR", Some(xdg.path().to_str().expect("utf8")));

        let resolved = resolve_socket_path(None, SocketMode::Runtime);
        assert_eq!(
            resolved,
            xdg.path().join(RUNTIME_SUBDIR).join(RUNTIME_SOCKET_NAME)
        );
    }

    #[test]
    fn default_runtime_dir_rejects_nonexistent_relative_symlink_and_non_private_xdg() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let _socket = EnvGuard::set(LIMUX_SOCKET_ENV, None);
        let _socket_path = EnvGuard::set(LIMUX_SOCKET_PATH_ENV, None);

        let expected_fallback = env::temp_dir().join(format!("limux-{}", current_euid()));
        let temp = TempDir::new().expect("tempdir");

        let missing = temp.path().join("missing-xdg");
        {
            let _xdg = EnvGuard::set("XDG_RUNTIME_DIR", Some(missing.to_str().expect("utf8")));
            assert_eq!(default_runtime_dir(), expected_fallback);
        }

        {
            let _xdg = EnvGuard::set("XDG_RUNTIME_DIR", Some("relative/xdg-runtime"));
            assert_eq!(default_runtime_dir(), expected_fallback);
        }

        let private_target = temp.path().join("private-target");
        std::fs::create_dir(&private_target).expect("create private target");
        std::fs::set_permissions(&private_target, PermissionsExt::from_mode(0o700))
            .expect("chmod 0700");
        let symlink_xdg = temp.path().join("symlink-xdg");
        std::os::unix::fs::symlink(&private_target, &symlink_xdg).expect("symlink xdg");
        {
            let _xdg = EnvGuard::set("XDG_RUNTIME_DIR", Some(symlink_xdg.to_str().expect("utf8")));
            assert_eq!(default_runtime_dir(), expected_fallback);
        }

        for mode in [0o755, 0o777] {
            let shared_xdg = temp.path().join(format!("shared-{mode:o}"));
            std::fs::create_dir(&shared_xdg).expect("create shared xdg");
            std::fs::set_permissions(&shared_xdg, PermissionsExt::from_mode(mode))
                .expect("chmod shared xdg");
            let _xdg = EnvGuard::set("XDG_RUNTIME_DIR", Some(shared_xdg.to_str().expect("utf8")));
            assert_eq!(
                default_runtime_dir(),
                expected_fallback,
                "mode {mode:o} must fall back"
            );
        }

        assert!(!is_usable_xdg_runtime_dir(
            &private_target,
            current_euid().wrapping_add(1)
        ));
    }

    #[test]
    fn debug_mode_defaults_to_per_uid_private_runtime_socket() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let _socket = EnvGuard::set(LIMUX_SOCKET_ENV, None);
        let _socket_path = EnvGuard::set(LIMUX_SOCKET_PATH_ENV, None);
        let _xdg = EnvGuard::set("XDG_RUNTIME_DIR", None);

        let expected_dir = env::temp_dir().join(format!("limux-{}", current_euid()));
        assert_eq!(
            resolve_socket_path(None, SocketMode::Debug),
            expected_dir.join(DEBUG_SOCKET_NAME)
        );
        assert_eq!(
            resolve_socket_path(None, SocketMode::Runtime),
            expected_dir.join(RUNTIME_SOCKET_NAME)
        );
    }

    #[test]
    fn ensure_private_dir_rejects_symlinks_wrong_owner_and_enforces_0700() {
        let temp = TempDir::new().expect("tempdir");
        let real_dir = temp.path().join("real");
        std::fs::create_dir(&real_dir).expect("create real dir");
        std::fs::set_permissions(&real_dir, PermissionsExt::from_mode(0o755)).expect("chmod 0755");

        ensure_private_dir(&real_dir).expect("tighten permissions");
        let mode = std::fs::metadata(&real_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, PRIVATE_DIR_MODE);

        let link_path = temp.path().join("symlink-dir");
        std::os::unix::fs::symlink(&real_dir, &link_path).expect("symlink");
        let err = ensure_private_dir(&link_path).expect_err("symlink must be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

        let owner_err = ensure_private_dir_for_uid(&real_dir, current_euid().wrapping_add(1))
            .expect_err("foreign owner must be rejected");
        assert_eq!(owner_err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn prepare_socket_path_locks_down_runtime_parent_dir() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let _socket = EnvGuard::set(LIMUX_SOCKET_ENV, None);
        let _socket_path = EnvGuard::set(LIMUX_SOCKET_PATH_ENV, None);
        let xdg = TempDir::new().expect("xdg runtime dir temp path");
        std::fs::set_permissions(xdg.path(), PermissionsExt::from_mode(0o700))
            .expect("chmod xdg 0700");
        let _xdg = EnvGuard::set("XDG_RUNTIME_DIR", Some(xdg.path().to_str().expect("utf8")));

        let path = resolve_socket_path(None, SocketMode::Runtime);
        prepare_socket_path(&path, SocketMode::Runtime, true).expect("prepare socket path");

        let mode = std::fs::metadata(path.parent().expect("socket parent"))
            .expect("parent metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, PRIVATE_DIR_MODE);
    }

    #[test]
    fn finalize_socket_permissions_sets_socket_mode() {
        let temp_dir = TempDir::new().expect("temp dir");
        let socket_path = temp_dir.path().join("limux.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind listener");

        finalize_socket_permissions(&socket_path, true).expect("set socket permissions");

        let mode = std::fs::metadata(&socket_path)
            .expect("socket metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, SOCKET_FILE_MODE);

        drop(listener);
    }

    #[test]
    fn prepare_socket_path_does_not_force_private_parent_for_allow_all() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let _socket = EnvGuard::set(LIMUX_SOCKET_ENV, None);
        let _socket_path = EnvGuard::set(LIMUX_SOCKET_PATH_ENV, None);
        let xdg = TempDir::new().expect("xdg runtime dir temp path");
        let _xdg = EnvGuard::set("XDG_RUNTIME_DIR", Some(xdg.path().to_str().expect("utf8")));

        let path = resolve_socket_path(None, SocketMode::Runtime);
        std::fs::create_dir_all(path.parent().expect("socket parent")).expect("create parent");
        std::fs::set_permissions(
            path.parent().expect("socket parent"),
            PermissionsExt::from_mode(0o755),
        )
        .expect("set parent permissions");

        prepare_socket_path(&path, SocketMode::Runtime, false).expect("prepare socket path");

        let mode = std::fs::metadata(path.parent().expect("socket parent"))
            .expect("parent metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn prepare_socket_path_refuses_to_overwrite_non_socket_path() {
        let temp_dir = TempDir::new().expect("temp dir");
        let socket_path = temp_dir.path().join("limux.sock");
        std::fs::write(&socket_path, b"not a socket").expect("write placeholder");

        let error = prepare_socket_path(&socket_path, SocketMode::Runtime, true)
            .expect_err("non-socket path should fail");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn prepare_socket_path_rejects_live_socket() {
        let temp_dir = TempDir::new().expect("temp dir");
        let socket_path = temp_dir.path().join("limux.sock");
        let _listener = UnixListener::bind(&socket_path).expect("bind listener");

        let error = prepare_socket_path(&socket_path, SocketMode::Runtime, true)
            .expect_err("live socket should fail");
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn prepare_socket_path_removes_stale_socket() {
        let temp_dir = TempDir::new().expect("temp dir");
        let socket_path = temp_dir.path().join("limux.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind listener");
        drop(listener);

        prepare_socket_path(&socket_path, SocketMode::Runtime, true)
            .expect("stale socket should be removed");
        assert!(!socket_path.exists());
    }
}
