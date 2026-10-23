//! Remote notifications use the SSH PTY, never a forwarded control socket.
use anyhow::{anyhow, bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn forced() -> bool {
    std::env::var("LIMUX_NOTIFY_OSC").is_ok_and(|v| matches!(v.as_str(), "1" | "true"))
}

pub fn remote() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some_and(|v| !v.is_empty())
}

pub fn socket_unavailable(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            )
        })
    })
}

fn field(value: &str, limit: usize, title: bool) -> String {
    let mut result = String::new();
    for ch in value.chars().filter(|ch| !ch.is_control()) {
        let ch = if title && ch == ';' { ' ' } else { ch };
        if result.len() + ch.len_utf8() > limit {
            break;
        }
        result.push(ch);
    }
    result
}

fn sequence(title: &str, body: &str) -> String {
    let title = field(title, 512, true);
    // Ghostty captures at most 2048 bytes, including the parser's trailing NUL.
    let body_limit = 2048 - "notify;".len() - title.len() - 1 - 1;
    format!(
        "\x1b]777;notify;{};{}\x07",
        title,
        field(body, body_limit, false)
    )
}

fn numeric_id(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

// The command and format strings are fixed. Bound time and captured output;
// use a temporary file so a stuck/flooding tmux cannot deadlock a pipe reader.
fn tmux(args: &[&str]) -> Result<String> {
    let output = tempfile::tempfile()?;
    let mut child = Command::new("tmux")
        .args(args)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(Stdio::null())
        .spawn()
        .context("start tmux notification lookup")?;
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("tmux notification lookup failed");
            }
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("tmux notification lookup timed out");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    use std::io::{Seek, SeekFrom};
    let mut output = output;
    output.seek(SeekFrom::Start(0))?;
    let mut text = String::new();
    output.take(8193).read_to_string(&mut text)?;
    if text.len() > 8192 {
        bail!("too many tmux notification clients");
    }
    Ok(text)
}

fn originating_session(context: &str) -> Result<(String, u32)> {
    let mut fields = context.rsplitn(3, ',');
    let session = fields.next().unwrap_or_default();
    let pid = fields.next().unwrap_or_default();
    let socket = fields.next().unwrap_or_default();
    if !numeric_id(session, "") || !numeric_id(pid, "") || socket.is_empty() {
        bail!(
            "missing or invalid TMUX session context; refusing to guess notification destination"
        );
    }
    let server_pid = pid
        .parse::<std::num::NonZeroU32>()
        .context("invalid TMUX server PID")?
        .get();
    Ok((format!("${session}"), server_pid))
}

fn client_ttys(pane: &str, context: &str) -> Result<Vec<String>> {
    if !numeric_id(pane, "%") {
        bail!("missing or invalid TMUX_PANE; refusing to guess notification destination");
    }
    let (session, server_pid) = originating_session(context)?;
    let panes = tmux(&[
        "list-panes",
        "-s",
        "-t",
        &session,
        "-F",
        "#{pid}:#{pane_id}",
    ])?;
    let expected_pane = format!("{server_pid}:{pane}");
    if !panes.lines().any(|candidate| candidate == expected_pane) {
        bail!("notification pane does not belong to its originating tmux session");
    }
    let clients = tmux(&["list-clients", "-t", &session, "-F", "#{client_tty}"])?;
    let mut paths = Vec::new();
    for tty in clients.lines() {
        if !numeric_id(tty, "/dev/pts/") {
            bail!("invalid tmux client terminal");
        }
        if !paths.iter().any(|path| path == tty) {
            paths.push(tty.to_string());
        }
    }
    if paths.is_empty() {
        bail!("notification session has no attached tmux client");
    }
    Ok(paths)
}

#[cfg(target_os = "linux")]
fn open_tty(path: &str) -> Result<File> {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    // Linux O_NOCTTY | O_NONBLOCK | O_NOFOLLOW. Never acquire a controlling
    // terminal, follow a symlink, create a file, or block on a stopped PTY.
    let file = OpenOptions::new()
        .write(true)
        .custom_flags(0x100 | 0x800 | 0x20000)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_char_device()
        || (path != "/dev/tty" && metadata.uid() != unsafe { getuid() })
    {
        bail!("notification target is not this user's terminal");
    }
    Ok(file)
}

#[cfg(not(target_os = "linux"))]
fn open_tty(_path: &str) -> Result<File> {
    bail!("remote terminal notifications currently require Linux");
}

pub fn emit(title: &str, body: &str) -> Result<()> {
    let paths = if std::env::var_os("TMUX").is_some_and(|v| !v.is_empty()) {
        // Agent hooks can lack /dev/tty. Keep the inherited session identity,
        // since linked windows can place the same pane in several sessions.
        client_ttys(
            &std::env::var("TMUX_PANE").unwrap_or_default(),
            &std::env::var("TMUX").context("invalid TMUX session context")?,
        )?
    } else {
        vec!["/dev/tty".to_string()]
    };
    let sequence = sequence(title, body);
    let mut delivered = false;
    let mut error = None;
    for path in paths {
        match open_tty(&path)
            .and_then(|mut tty| tty.write_all(sequence.as_bytes()).map_err(Into::into))
        {
            Ok(()) => delivered = true,
            Err(err) => error = Some(err),
        }
    }
    if delivered {
        Ok(())
    } else {
        Err(error.unwrap_or_else(|| anyhow!("no notification terminal")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback_is_not_an_authorization_or_protocol_bypass() {
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::ConnectionRefused,
        ] {
            assert!(socket_unavailable(
                &anyhow::Error::from(std::io::Error::from(kind)).context("connect")
            ));
        }
        assert!(!socket_unavailable(
            &std::io::Error::from(std::io::ErrorKind::PermissionDenied).into()
        ));
        assert!(!socket_unavailable(&anyhow!(
            "server rejected notification"
        )));
    }
    #[test]
    fn osc_fields_are_bounded_and_cannot_inject_controls() {
        assert_eq!(
            sequence("a;b\x1b\x07\n", "c;d\u{009c}"),
            "\x1b]777;notify;a b;c;d\x07"
        );
        let result = sequence(&"é".repeat(1000), &"界".repeat(2000));
        let payload = result
            .strip_prefix("\x1b]777;")
            .unwrap()
            .strip_suffix('\x07')
            .unwrap();
        assert!(payload.len() < 2048);
        assert_eq!(result.matches('\x1b').count(), 1);
        assert_eq!(result.matches('\x07').count(), 1);
    }
    #[test]
    fn session_and_terminal_identifiers_are_strict() {
        for (value, prefix) in [("%0", "%"), ("$17", "$"), ("/dev/pts/42", "/dev/pts/")] {
            assert!(numeric_id(value, prefix));
        }
        for value in [
            "/dev/tty",
            "/dev/pts/../1",
            "/dev/pts/1\n",
            "/dev/pts/",
            "/tmp/output",
        ] {
            assert!(!numeric_id(value, "/dev/pts/"));
        }
        assert!(!numeric_id("-a", "%"));
        assert!(!numeric_id("$1;cmd", "$"));
    }
    #[test]
    fn tmux_context_keeps_the_explicit_session_and_socket_commas() {
        assert_eq!(
            originating_session("/tmp/tmux-1000/default,123,0").unwrap(),
            ("$0".to_string(), 123)
        );
        assert_eq!(
            originating_session("/tmp/custom,socket,123,42").unwrap(),
            ("$42".to_string(), 123)
        );
        assert_eq!(
            originating_session("relative.sock,123,0").unwrap(),
            ("$0".to_string(), 123)
        );
        for context in [
            "",
            "fixture",
            "/tmp/socket,123",
            "/tmp/socket,123,$0",
            "/tmp/socket,0,0",
            "/tmp/socket,000,0",
            "/tmp/socket,4294967296,0",
            "/tmp/socket,pid,0",
            ",123,0",
            "/tmp/socket,123,0;cmd",
        ] {
            assert!(originating_session(context).is_err(), "{context}");
        }
    }
    #[test]
    fn refuses_regular_files_and_symlinks() {
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(open_tty(file.path().to_str().unwrap()).is_err());
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("tty");
        std::os::unix::fs::symlink(file.path(), &link).unwrap();
        assert!(open_tty(link.to_str().unwrap()).is_err());
        assert_eq!(std::fs::read(file.path()).unwrap(), b"");
    }
}
