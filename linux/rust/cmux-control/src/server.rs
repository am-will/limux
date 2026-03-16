use std::io;
use std::path::Path;

use cmux_protocol::{parse_v1_command_envelope, V2Request, V2Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::Dispatcher;

pub async fn run_server<P: AsRef<Path>>(socket_path: P, dispatcher: Dispatcher) -> io::Result<()> {
    let socket_path = socket_path.as_ref();
    if socket_path.exists() {
        std::fs::remove_file(socket_path)?;
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(socket_path)?;
    serve(listener, dispatcher).await
}

pub async fn serve(listener: UnixListener, dispatcher: Dispatcher) -> io::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let dispatcher = dispatcher.clone();

        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, dispatcher).await {
                eprintln!("connection error: {error}");
            }
        });
    }
}

pub async fn handle_connection(stream: UnixStream, dispatcher: Dispatcher) -> io::Result<()> {
    let (reader_half, mut writer_half) = stream.into_split();
    let mut reader = BufReader::new(reader_half);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).await?;

        if bytes_read == 0 {
            return Ok(());
        }

        let incoming = line.trim_end_matches(['\n', '\r']);
        if incoming.is_empty() {
            continue;
        }

        let response = match parse_request(incoming) {
            Ok(request) => dispatcher.dispatch(request).await,
            Err(message) => V2Response::error(None, -32700, message, None),
        };

        let mut payload = serde_json::to_string(&response)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        payload.push('\n');

        writer_half.write_all(payload.as_bytes()).await?;
        writer_half.flush().await?;
    }
}

fn parse_request(incoming: &str) -> Result<V2Request, String> {
    if let Ok(request) = serde_json::from_str::<V2Request>(incoming) {
        return Ok(request);
    }

    match parse_v1_command_envelope(incoming) {
        Ok(v1) => Ok(v1.into_v2_request(None)),
        Err(error) => Err(format!("invalid request payload: {error}")),
    }
}
