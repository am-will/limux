use std::time::Duration;

use cmux_control::{server, Dispatcher};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

#[tokio::test]
async fn socket_roundtrip_for_ping() {
    let temp_dir = tempfile::tempdir().expect("temp dir should be created");
    let socket_path = temp_dir.path().join("cmux-control.sock");

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
    let socket_path = temp_dir.path().join("cmux-control.sock");

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
