#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

#[test]
fn host_replaces_cli_and_preserves_stdin_and_exit_status() {
    let directory = tempfile::tempdir().unwrap();
    let host = directory.path().join("host");
    let pid_file = directory.path().join("host.pid");
    fs::write(
        &host,
        "#!/bin/sh\nprintf '%s' \"$$\" > \"$LIMUX_TEST_HOST_PID\"\nIFS= read -r input\nprintf '%s' \"$input\"\nexit 37\n",
    )
    .unwrap();
    fs::set_permissions(&host, fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_limux-cli"))
        .env("LIMUX_HOST_BIN", &host)
        .env("LIMUX_TEST_HOST_PID", &pid_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let cli_pid = child.id();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"launcher input is preserved\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(fs::read_to_string(pid_file).unwrap(), cli_pid.to_string());
    assert_eq!(output.status.code(), Some(37));
    assert_eq!(output.stdout, b"launcher input is preserved");
}
