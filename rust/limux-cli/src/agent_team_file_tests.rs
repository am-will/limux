use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

fn spawn(command: &str, cwd: &Path) -> Child {
    Command::new("/bin/sh")
        .args(["-c", command])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn finish(mut child: Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("gated startup did not terminate");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn dropped_or_failed_publication_never_launches_a_waiting_agent() {
    for concurrent_file in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        let pending = PendingInstructions::new(&path).unwrap();
        let child = spawn(
            &pending.pane_command(Some("/bin/echo launched")),
            dir.path(),
        );
        if concurrent_file {
            fs::write(&path, "Someone else's instructions").unwrap();
            assert!(pending.publish("Our instructions").is_err());
        } else {
            drop(pending);
        }
        let output = finish(child);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn launch_uses_parent_shell_function_after_publication() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("user's project");
    fs::create_dir(&cwd).unwrap();
    let pending = PendingInstructions::new(&cwd.join("AGENTS.md")).unwrap();
    let command = format!(
        "parent_shell_agent() {{ printf 'launched:%s\\n' \"$PWD\"; }}; {}",
        pending.pane_command(Some("parent_shell_agent"))
    );
    let child = spawn(&command, dir.path());

    pending.publish("Team instructions").unwrap();

    let output = finish(child);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("launched:{}\n", cwd.display())
    );
}

#[test]
fn no_launch_uses_the_inherited_shell_in_the_requested_directory() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("user's project");
    fs::create_dir(&cwd).unwrap();
    let shell = dir.path().join("user's shell");
    fs::write(&shell, "#!/bin/sh\npwd\n").unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
    let pending = PendingInstructions::new(&cwd.join("AGENTS.md")).unwrap();
    let output = Command::new("/bin/sh")
        .args(["-c", &pending.pane_command(None)])
        .env("SHELL", &shell)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        cwd.to_str().unwrap()
    );
    assert!(!cwd.join("AGENTS.md").exists());
}

#[tokio::test]
async fn socket_created_agents_wait_for_all_peer_ids_in_the_requested_cwd() {
    use crate::{run_agent_team, Client};
    use limux_protocol::{V2Request, V2Response};
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("user's project B");
    fs::create_dir(&cwd).unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    for name in ["codex", "claude"] {
        let stub = bin.join(name);
        fs::write(&stub, "#!/bin/sh\npwd\ncat AGENTS.md\n").unwrap();
        fs::set_permissions(stub, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let socket = dir.path().join("host.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let requested_cwd = cwd.clone();
    let workspace_cwd = dir.path().to_path_buf();
    let server = tokio::spawn(async move {
        let mut children = Vec::new();
        while children.len() < 2 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            let request: V2Request = serde_json::from_str(&line).unwrap();
            let result = match request.method.as_str() {
                "workspace.current" => json!({"workspace_id": "ws"}),
                "workspace.list" => json!({"workspaces": [{"workspace_id": "ws", "name": "A"}]}),
                "surface.list" => {
                    json!({"surfaces": [{"pane_id": "1", "surface_id": "1:orchestrator", "focused": true}]})
                }
                "pane.create" => {
                    assert!(!requested_cwd.join("AGENTS.md").exists());
                    let command = request.params["command"].as_str().unwrap();
                    let mut child = Command::new("/bin/sh")
                        .args(["-c", command])
                        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                        .current_dir(&workspace_cwd)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                        .unwrap();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    assert!(
                        child.try_wait().unwrap().is_none(),
                        "agent ran before publication"
                    );
                    children.push(child);
                    let pane = children.len() + 1;
                    json!({"pane_id": pane.to_string(), "surface_id": format!("{pane}:agent")})
                }
                method => panic!("unexpected request: {method}"),
            };
            let response = V2Response::success(request.id, result);
            let response = format!("{}\n", serde_json::to_string(&response).unwrap());
            stream
                .get_mut()
                .write_all(response.as_bytes())
                .await
                .unwrap();
        }
        children
    });
    let mut client = Client::new(socket);
    let response = run_agent_team(
        &mut client,
        &["--cwd".into(), cwd.to_string_lossy().into_owned()],
    )
    .await
    .unwrap();
    assert_eq!(response["cwd"], cwd.to_str().unwrap());
    for child in server.await.unwrap() {
        let output = finish(child);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8(output.stdout).unwrap();
        assert!(output.starts_with(&format!("{}\n", cwd.display())));
        assert!(output.contains("| `codex` | `2` | `2:agent` |"));
        assert!(output.contains("| `claude` | `3` | `3:agent` |"));
    }
}

#[test]
fn existing_instructions_are_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("AGENTS.md");
    let original = "# Project instructions\nKeep my local policies.\n";
    fs::write(&path, original).unwrap();
    assert!(PendingInstructions::new(&path).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), original);
}

#[test]
fn concurrent_creation_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("AGENTS.md");
    let pending = PendingInstructions::new(&path).unwrap();
    fs::write(&path, "Created while panes were starting").unwrap();
    assert!(pending.publish("Generated protocol").is_err());
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "Created while panes were starting"
    );
}

#[test]
fn published_instructions_are_preserved_on_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("AGENTS.md");
    PendingInstructions::new(&path)
        .unwrap()
        .publish("Generated protocol with edited policies")
        .unwrap();
    let published = fs::read_to_string(&path).unwrap();
    assert!(published.starts_with("Generated protocol with edited policies"));
    assert!(PendingInstructions::new(&path).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), published);
}

#[test]
fn abandoned_stage_leaves_no_instructions_or_temporary_files() {
    let dir = tempfile::tempdir().unwrap();
    drop(PendingInstructions::new(&dir.path().join("AGENTS.md")).unwrap());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn dangling_symlink_is_not_followed_or_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("AGENTS.md");
    let target = dir.path().join("missing-target");
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(PendingInstructions::new(&path).is_err());
    assert_eq!(fs::read_link(path).unwrap(), target);
    assert!(!target.exists());
}
