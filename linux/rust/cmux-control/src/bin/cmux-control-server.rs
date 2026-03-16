use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use cmux_control::socket_path::{resolve_socket_path, SocketMode};

#[derive(Debug, Parser)]
#[command(
    name = "cmux-control-server",
    about = "cmux control-plane unix socket server"
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
    let socket = resolve_socket_path(args.socket, args.socket_mode.into());
    if std::env::var("CMUX_DEBUG_LOG")
        .ok()
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        if let Some(log_path) = derive_debug_log_path(&socket) {
            std::env::set_var("CMUX_DEBUG_LOG", &log_path);
            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path);
        }
    }
    let dispatcher = cmux_control::Dispatcher::new();
    cmux_control::server::run_server(&socket, dispatcher).await?;
    Ok(())
}

fn derive_debug_log_path(socket: &std::path::Path) -> Option<PathBuf> {
    let file_name = socket.file_name()?.to_string_lossy();
    let stem = file_name.strip_suffix(".sock").unwrap_or(&file_name);
    let log_name = format!("{stem}.log");
    let parent = socket
        .parent()
        .unwrap_or_else(|| std::path::Path::new("/tmp"));
    Some(parent.join(log_name))
}
