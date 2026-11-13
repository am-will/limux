use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use limux_control::socket_path::{
    default_runtime_dir, ensure_private_dir, resolve_socket_path, SocketMode,
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
        if let Some(log_path) = derive_debug_log_path(&socket) {
            std::env::set_var("LIMUX_DEBUG_LOG", &log_path);
            if let Some(parent) = log_path.parent() {
                if parent == default_runtime_dir().as_path() {
                    let _ = ensure_private_dir(parent);
                } else {
                    let _ = std::fs::create_dir_all(parent);
                }
            }
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&log_path);
        }
    }
    let dispatcher = limux_control::Dispatcher::new();
    limux_control::server::run_server(&socket, socket_mode, dispatcher).await?;
    Ok(())
}

fn derive_debug_log_path(socket: &std::path::Path) -> Option<PathBuf> {
    let file_name = socket.file_name()?.to_string_lossy();
    let stem = file_name.strip_suffix(".sock").unwrap_or(&file_name);
    let log_name = format!("{stem}.log");
    let fallback_dir = default_runtime_dir();
    let parent = socket
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(fallback_dir.as_path());
    Some(parent.join(log_name))
}
