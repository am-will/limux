use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use limux_control::socket_path::{
    current_euid, default_runtime_dir, ensure_private_dir_for_uid, resolve_socket_path, SocketMode,
};

#[derive(Debug, Parser)]
#[command(
    name = "limux-control-server",
    about = "limux control-plane unix socket server"
)]
struct Args {
    #[arg(long)]
    socket: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = CliSocketMode::Runtime)]
    socket_mode: CliSocketMode,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliSocketMode {
    Runtime,
    Debug,
}

impl From<CliSocketMode> for SocketMode {
    fn from(value: CliSocketMode) -> Self {
        match value {
            CliSocketMode::Runtime => SocketMode::Runtime,
            CliSocketMode::Debug => SocketMode::Debug,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let socket_mode: SocketMode = args.socket_mode.into();
    let socket = resolve_socket_path(args.socket, socket_mode);
    if std::env::var("LIMUX_DEBUG_LOG")
        .ok()
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        let runtime_dir = default_runtime_dir();
        init_default_debug_log_with(&socket, &runtime_dir, current_euid(), |log_path| {
            std::env::set_var("LIMUX_DEBUG_LOG", log_path);
        })?;
    }
    let dispatcher = limux_control::Dispatcher::new();
    limux_control::server::run_server(&socket, socket_mode, dispatcher).await?;
    Ok(())
}

fn init_default_debug_log_with(
    socket: &Path,
    runtime_dir: &Path,
    expected_uid: u32,
    set_env: impl FnOnce(&Path),
) -> io::Result<Option<PathBuf>> {
    let Some(log_path) = derive_debug_log_path_in(socket, runtime_dir) else {
        return Ok(None);
    };
    prepare_debug_log_file(&log_path, runtime_dir, expected_uid)?;
    set_env(&log_path);
    Ok(Some(log_path))
}

fn prepare_debug_log_file(
    log_path: &Path,
    runtime_dir: &Path,
    expected_uid: u32,
) -> io::Result<()> {
    if let Some(parent) = log_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        if parent == runtime_dir {
            ensure_private_dir_for_uid(parent, expected_uid)?;
        } else {
            std::fs::create_dir_all(parent)?;
        }
    }
    if let Ok(meta) = std::fs::symlink_metadata(log_path) {
        if meta.file_type().is_symlink() || !meta.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "refusing symlink or non-regular debug log {}",
                    log_path.display()
                ),
            ));
        }
        if meta.uid() != expected_uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "refusing debug log {} owned by uid={} (expected uid={expected_uid})",
                    log_path.display(),
                    meta.uid()
                ),
            ));
        }
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(log_path)?;
    let meta = file.metadata()?;
    if !meta.file_type().is_file() || meta.uid() != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "refusing non-regular or foreign-owned debug log {}",
                log_path.display()
            ),
        ));
    }
    Ok(())
}

fn derive_debug_log_path_in(socket: &Path, fallback_dir: &Path) -> Option<PathBuf> {
    let file_name = socket.file_name()?.to_string_lossy();
    let stem = file_name.strip_suffix(".sock").unwrap_or(&file_name);
    let log_name = format!("{stem}.log");
    let parent = socket
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(fallback_dir);
    Some(parent.join(log_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    #[test]
    fn init_default_debug_log_creates_private_log_and_sets_env_after_open() {
        let temp = TempDir::new().expect("tempdir");
        let runtime_dir = temp.path().join("limux");
        let socket = runtime_dir.join("limux.sock");
        let mut recorded_env = None;

        let log_path = init_default_debug_log_with(&socket, &runtime_dir, current_euid(), |path| {
            recorded_env = Some(path.to_path_buf());
        })
        .expect("init debug log")
        .expect("derived log path");

        assert_eq!(log_path, runtime_dir.join("limux.log"));
        assert_eq!(recorded_env.as_deref(), Some(log_path.as_path()));
        let dir_mode = std::fs::symlink_metadata(&runtime_dir)
            .expect("runtime dir metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        let file_mode = std::fs::symlink_metadata(&log_path)
            .expect("log metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
    }

    #[test]
    fn init_default_debug_log_rejects_symlink_and_wrong_owner_runtime_dir() {
        let temp = TempDir::new().expect("tempdir");
        let real_dir = temp.path().join("real");
        std::fs::create_dir(&real_dir).expect("create real dir");
        let symlink_runtime = temp.path().join("limux-symlink");
        std::os::unix::fs::symlink(&real_dir, &symlink_runtime).expect("symlink runtime");
        let symlink_socket = symlink_runtime.join("limux.sock");

        let mut env_set = false;
        let err =
            init_default_debug_log_with(&symlink_socket, &symlink_runtime, current_euid(), |_| {
                env_set = true;
            })
            .expect_err("symlink runtime dir must fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!env_set);
        assert!(!real_dir.join("limux.log").exists());

        let foreign_runtime = temp.path().join("foreign-runtime");
        std::fs::create_dir(&foreign_runtime).expect("create foreign dir");
        let foreign_socket = foreign_runtime.join("limux.sock");
        let err = init_default_debug_log_with(
            &foreign_socket,
            &foreign_runtime,
            current_euid().wrapping_add(1),
            |_| {
                env_set = true;
            },
        )
        .expect_err("wrong-owner runtime dir must fail");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(!env_set);
        assert!(!foreign_runtime.join("limux.log").exists());
    }

    #[test]
    fn init_default_debug_log_rejects_symlink_log_targets() {
        let temp = TempDir::new().expect("tempdir");
        let runtime_dir = temp.path().join("limux");
        std::fs::create_dir(&runtime_dir).expect("create runtime dir");
        std::fs::set_permissions(&runtime_dir, PermissionsExt::from_mode(0o700))
            .expect("chmod 0700");
        let socket = runtime_dir.join("limux.sock");
        let log_link = runtime_dir.join("limux.log");

        let dangling_target = temp.path().join("victim-new.txt");
        std::os::unix::fs::symlink(&dangling_target, &log_link).expect("dangling symlink");
        let mut env_set = false;
        assert!(
            init_default_debug_log_with(&socket, &runtime_dir, current_euid(), |_| {
                env_set = true;
            })
            .is_err()
        );
        assert!(!env_set);
        assert!(!dangling_target.exists());

        std::fs::remove_file(&log_link).expect("remove dangling link");
        let existing_target = temp.path().join("victim-existing.txt");
        std::fs::write(&existing_target, b"keep").expect("write victim");
        std::os::unix::fs::symlink(&existing_target, &log_link).expect("existing symlink");
        assert!(
            init_default_debug_log_with(&socket, &runtime_dir, current_euid(), |_| {
                env_set = true;
            })
            .is_err()
        );
        assert!(!env_set);
        assert_eq!(std::fs::read(&existing_target).unwrap(), b"keep");
    }
}
