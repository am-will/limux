use std::time::Duration;

use limux_control::request_io::MAX_REQUEST_LEN;
use limux_control::socket_path::SocketMode;
use limux_control::{server, Dispatcher};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

#[tokio::test]
async fn socket_roundtrip_for_ping() {
    let temp_dir = tempfile::tempdir().expect("temp dir should be created");
    let socket_path = temp_dir.path().join("limux-control.sock");

    let listener = UnixListener::bind(&socket_path).expect("listener should bind");
    let server_task = tokio::spawn(async move {
        let dispatcher = Dispatcher::new();
        let _ = server::serve(listener, dispatcher).await;
    });

    let mut attempts = 0;
    let stream = loop {
        match UnixStream::connect(&socket_path).await {
            Ok(stream) => break stream,
            Err(error) if attempts < 20 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(10)).await;
                let _ = error;
            }
            Err(error) => panic!("failed to connect to server socket: {error}"),
        }
    };

    let (reader_half, mut writer_half) = stream.into_split();
    let mut reader = BufReader::new(reader_half);

    writer_half
        .write_all(b"{\"id\":\"1\",\"method\":\"system.ping\",\"params\":{}}\n")
        .await
        .expect("request should write");
    writer_half.flush().await.expect("request should flush");

    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .await
        .expect("response should read");

    let response: Value =
        serde_json::from_str(response_line.trim()).expect("response should be valid json");
    assert_eq!(response["id"], "1");
    assert_eq!(response["result"]["pong"], true);

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test]
async fn socket_roundtrip_accepts_v1_envelope() {
    let temp_dir = tempfile::tempdir().expect("temp dir should be created");
    let socket_path = temp_dir.path().join("limux-control.sock");

    let listener = UnixListener::bind(&socket_path).expect("listener should bind");
    let server_task = tokio::spawn(async move {
        let dispatcher = Dispatcher::new();
        let _ = server::serve(listener, dispatcher).await;
    });

    let mut attempts = 0;
    let stream = loop {
        match UnixStream::connect(&socket_path).await {
            Ok(stream) => break stream,
            Err(error) if attempts < 20 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(10)).await;
                let _ = error;
            }
            Err(error) => panic!("failed to connect to server socket: {error}"),
        }
    };

    let (reader_half, mut writer_half) = stream.into_split();
    let mut reader = BufReader::new(reader_half);

    writer_half
        .write_all(b"{\"command\":\"system.ping\",\"params\":{}}\n")
        .await
        .expect("request should write");
    writer_half.flush().await.expect("request should flush");

    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .await
        .expect("response should read");

    let response: Value =
        serde_json::from_str(response_line.trim()).expect("response should be valid json");
    assert_eq!(response["result"]["pong"], true);

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test]
async fn run_server_refuses_to_overwrite_non_socket_path() {
    let temp_dir = tempfile::tempdir().expect("temp dir should be created");
    let socket_path = temp_dir.path().join("limux-control.sock");
    std::fs::write(&socket_path, b"not a socket").expect("placeholder file");

    let error = server::run_server(&socket_path, SocketMode::Runtime, Dispatcher::new())
        .await
        .expect_err("non-socket path should be rejected");

    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        std::fs::read(&socket_path).expect("placeholder file should remain"),
        b"not a socket"
    );
}

#[tokio::test]
async fn run_server_reports_addr_in_use_for_live_socket() {
    let temp_dir = tempfile::tempdir().expect("temp dir should be created");
    let socket_path = temp_dir.path().join("limux-control.sock");

    let listener = UnixListener::bind(&socket_path).expect("listener should bind");
    let server_task = tokio::spawn(async move {
        let dispatcher = Dispatcher::new();
        let _ = server::serve(listener, dispatcher).await;
    });

    let mut attempts = 0;
    loop {
        match UnixStream::connect(&socket_path).await {
            Ok(_) => break,
            Err(error) if attempts < 20 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(10)).await;
                let _ = error;
            }
            Err(error) => panic!("failed to connect to server socket: {error}"),
        }
    }

    let error = server::run_server(&socket_path, SocketMode::Runtime, Dispatcher::new())
        .await
        .expect_err("live socket should report addr in use");
    assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test]
async fn oversized_request_closes_connection_without_response() {
    let temp_dir = tempfile::tempdir().expect("temp dir should be created");
    let socket_path = temp_dir.path().join("limux-control.sock");

    let listener = UnixListener::bind(&socket_path).expect("listener should bind");
    let server_task = tokio::spawn(async move {
        let dispatcher = Dispatcher::new();
        let _ = server::serve(listener, dispatcher).await;
    });

    let stream = UnixStream::connect(&socket_path)
        .await
        .expect("connect stream");
    let (reader_half, mut writer_half) = stream.into_split();
    let mut reader = BufReader::new(reader_half);

    let mut request = vec![b'a'; MAX_REQUEST_LEN + 1];
    request.push(b'\n');
    writer_half
        .write_all(&request)
        .await
        .expect("oversized request should write");
    writer_half
        .flush()
        .await
        .expect("oversized request should flush");

    let mut response_line = String::new();
    let bytes_read = reader
        .read_line(&mut response_line)
        .await
        .expect("response read should succeed");
    assert_eq!(bytes_read, 0);

    server_task.abort();
    let _ = server_task.await;
}
