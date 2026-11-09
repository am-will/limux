//! Bounded binary transfers through OpenSSH, and Flatpak host image staging.
//! Neither path keeps a local staging file or launches a login shell.
use crate::ssh_hosts::SshTarget;
use gio::prelude::*;
use glib::translate::{IntoGlib, ToGlibPtr};
use gtk4::{gio, glib};
use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::rc::Rc;
use std::time::Duration;

pub const MAX_PNG_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_REPLY_BYTES: usize = 256;
const UPLOAD_SCRIPT: &str = r#"set -eu
umask 077
dir=$(mktemp -d /tmp/limux-paste.XXXXXXXXXX)
trap 'rm -f "$dir/image.png"; rmdir "$dir"' 0
trap 'exit 1' HUP INT TERM
cat > "$dir/image.png"
test -s "$dir/image.png"
printf '%s\n' "$dir/image.png"
trap - 0 HUP INT TERM"#;

fn arguments(target: &SshTarget, flatpak: bool) -> Vec<String> {
    arguments_for_script(target, flatpak, UPLOAD_SCRIPT)
}

fn arguments_for_script(target: &SshTarget, flatpak: bool, script: &str) -> Vec<String> {
    let mut args = if flatpak {
        vec![
            "/app/bin/host-spawn".into(),
            "--no-pty".into(),
            "--cwd=/".into(),
            "--env=SSH_ASKPASS_REQUIRE".into(),
        ]
    } else {
        Vec::new()
    };
    args.extend(
        [
            "ssh",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
        ]
        .into_iter()
        .map(str::to_string),
    );
    if let Some(port) = target.port() {
        args.extend(["-p".into(), port.to_string()]);
    }
    args.extend([
        "--".into(),
        target.destination().into(),
        format!("/bin/sh -c '{}'", script.replace('\'', "'\\''")),
    ]);
    args
}

unsafe extern "C" {
    fn setsid() -> std::ffi::c_int;
    fn _exit(status: std::ffi::c_int) -> !;
}

unsafe extern "C" fn detach_upload_process(_data: glib::ffi::gpointer) {
    // Only async-signal-safe operations are allowed between fork and exec.
    unsafe {
        if setsid() < 0 {
            _exit(127);
        }
    }
}

fn relay_diagnostics(stderr: &gio::InputStream) {
    let log: gio::OutputStream = gio::WriteOutputStream::new(std::io::stderr()).upcast();
    // GIO retains both streams and copies bounded chunks with worker-thread
    // writes, so a slow launch log never blocks the GTK main loop.
    unsafe {
        gio::ffi::g_output_stream_splice_async(
            log.to_glib_none().0,
            stderr.to_glib_none().0,
            (gio::OutputStreamSpliceFlags::CLOSE_SOURCE
                | gio::OutputStreamSpliceFlags::CLOSE_TARGET)
                .into_glib(),
            glib::Priority::DEFAULT.into_glib(),
            std::ptr::null_mut(),
            None,
            std::ptr::null_mut(),
        );
    }
}

#[derive(Clone, Copy)]
struct TransferMessages {
    start: &'static str,
    timeout: &'static str,
    finish: &'static str,
    send: &'static str,
    response: &'static str,
    failed: &'static str,
    cancelled: &'static str,
}

const SSH_TRANSFER: TransferMessages = TransferMessages {
    start: "Could not start SSH upload",
    timeout: "SSH transfer timed out",
    finish: "Could not finish SSH transfer",
    send: "Could not send data over SSH",
    response: "Invalid or failed SSH upload response",
    failed: "SSH upload failed. Check keys/agent, known_hosts, and the launch log.",
    cancelled: "SSH transfer cancelled",
};

const HOST_STAGE: TransferMessages = TransferMessages {
    start: "Could not save the clipboard image",
    timeout: "Saving the clipboard image timed out",
    finish: "Could not finish saving the clipboard image",
    send: "Could not write the clipboard image",
    response: "Invalid image staging response",
    failed: "Could not save the clipboard image",
    cancelled: "Image paste cancelled",
};

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

pub(crate) fn validate_clipboard_png(png: &[u8]) -> Result<(), String> {
    if png.len() > MAX_PNG_BYTES || !png.starts_with(PNG_MAGIC) {
        Err("Clipboard image is invalid or exceeds 32 MiB".into())
    } else {
        Ok(())
    }
}

/// Accept only `/tmp/limux-paste.<10 alphanumeric>/image.png`.
pub(crate) fn validated_paste_path(path: &str) -> Option<String> {
    let token = path
        .strip_prefix("/tmp/limux-paste.")
        .and_then(|value| value.strip_suffix("/image.png"))
        .filter(|value| {
            value.len() == 10 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })?;
    Some(format!("/tmp/limux-paste.{token}/image.png"))
}

fn parse_paste_reply(reply: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(reply).ok()?;
    let path = text.strip_suffix('\n')?;
    validated_paste_path(path)
}

fn remote_path(reply: &[u8]) -> Result<String, String> {
    parse_paste_reply(reply).ok_or_else(|| "Invalid SSH upload response".into())
}

fn host_stage_path(reply: &[u8]) -> Result<String, String> {
    parse_paste_reply(reply).ok_or_else(|| "Invalid image staging response".into())
}

pub(crate) fn host_stage_arguments() -> Vec<String> {
    vec![
        "/app/bin/host-spawn".into(),
        "--no-pty".into(),
        "--cwd=/".into(),
        "/bin/sh".into(),
        "-c".into(),
        UPLOAD_SCRIPT.into(),
    ]
}

type Completion = Box<dyn FnOnce(Result<String, String>)>;
pub struct Upload {
    process: gio::Subprocess,
    cancellable: gio::Cancellable,
    completion: RefCell<Option<Completion>>,
    timer: RefCell<Option<glib::SourceId>>,
    exited: Rc<Cell<bool>>,
    validate_reply: fn(&[u8]) -> Result<String, String>,
    messages: &'static TransferMessages,
    /// When set, a path that was printed and then not delivered is deleted.
    host_cleanup: Option<fn(&str)>,
    abandoned: Cell<bool>,
}

impl Upload {
    pub fn start(
        target: &SshTarget,
        png: glib::Bytes,
        flatpak: bool,
        completion: impl FnOnce(Result<String, String>) + 'static,
    ) -> Result<Rc<Self>, String> {
        validate_clipboard_png(png.as_ref())?;
        let args = arguments(target, flatpak);
        Self::spawn(&args, png, Duration::from_secs(30), Box::new(completion))
    }

    pub fn stage_on_host(
        png: glib::Bytes,
        cleanup: fn(&str),
        completion: impl FnOnce(Result<String, String>) + 'static,
    ) -> Result<Rc<Self>, String> {
        validate_clipboard_png(png.as_ref())?;
        Self::spawn_with_reply(
            &host_stage_arguments(),
            png,
            Duration::from_secs(30),
            Box::new(completion),
            host_stage_path,
            &HOST_STAGE,
            Some(cleanup),
        )
    }

    pub fn install_helper(
        target: &SshTarget,
        bytes: glib::Bytes,
        script: &str,
        flatpak: bool,
        completion: impl FnOnce(Result<String, String>) + 'static,
    ) -> Result<Rc<Self>, String> {
        if bytes.is_empty() || bytes.len() > 64 * 1024 * 1024 {
            return Err("Helper exceeds 64 MiB".into());
        }
        let args = arguments_for_script(target, flatpak, script);
        Self::spawn_with_reply(
            &args,
            bytes,
            Duration::from_secs(60),
            Box::new(completion),
            |reply| {
                if reply == b"LIMUX_HELPER_INSTALLED\n" {
                    Ok("Helper installed".into())
                } else {
                    Err("Invalid helper installation response; inspect the launch log".into())
                }
            },
            &SSH_TRANSFER,
            None,
        )
    }

    fn spawn(
        args: &[String],
        png: glib::Bytes,
        timeout: Duration,
        completion: Completion,
    ) -> Result<Rc<Self>, String> {
        Self::spawn_with_reply(
            args,
            png,
            timeout,
            completion,
            remote_path,
            &SSH_TRANSFER,
            None,
        )
    }

    fn spawn_with_reply(
        args: &[String],
        png: glib::Bytes,
        timeout: Duration,
        completion: Completion,
        validate_reply: fn(&[u8]) -> Result<String, String>,
        messages: &'static TransferMessages,
        host_cleanup: Option<fn(&str)>,
    ) -> Result<Rc<Self>, String> {
        let launcher = gio::SubprocessLauncher::new(
            gio::SubprocessFlags::STDIN_PIPE
                | gio::SubprocessFlags::STDOUT_PIPE
                | gio::SubprocessFlags::STDERR_PIPE,
        );
        launcher.setenv("SSH_ASKPASS_REQUIRE", "never", true);
        // ProxyJump SSH children do not inherit the parent's BatchMode option.
        // Remove the GUI's controlling terminal instead of relying on options.
        unsafe {
            gio::ffi::g_subprocess_launcher_set_child_setup(
                launcher.to_glib_none().0,
                Some(detach_upload_process),
                std::ptr::null_mut(),
                None,
            );
        }
        let args: Vec<_> = args.iter().map(OsStr::new).collect();
        let process = launcher
            .spawn(&args)
            .map_err(|err| format!("{}: {err}", messages.start))?;
        // Flatpak HostCommand starts a new session but can acquire a terminal
        // from any standard descriptor. Piping stderr keeps host SSH detached.
        relay_diagnostics(&process.stderr_pipe().expect("stderr pipe requested"));
        let upload = Rc::new(Self {
            process,
            cancellable: gio::Cancellable::new(),
            completion: RefCell::new(Some(completion)),
            timer: RefCell::new(None),
            exited: Rc::new(Cell::new(false)),
            validate_reply,
            messages,
            host_cleanup,
            abandoned: Cell::new(false),
        });
        let exited = upload.exited.clone();
        upload
            .process
            .wait_async(gio::Cancellable::NONE, move |_| exited.set(true));
        let weak = Rc::downgrade(&upload);
        *upload.timer.borrow_mut() = Some(glib::timeout_add_local_once(timeout, move || {
            if let Some(upload) = weak.upgrade() {
                upload.timer.borrow_mut().take();
                upload.finish(Err(upload.messages.timeout.into()));
            }
        }));
        let stdin = upload.process.stdin_pipe().expect("stdin pipe requested");
        let next = upload.clone();
        stdin.write_all_async(
            png,
            glib::Priority::DEFAULT,
            Some(&upload.cancellable),
            move |result| match result {
                Ok((bytes, written, None)) if written == bytes.len() => {
                    let stdin = next.process.stdin_pipe().expect("stdin pipe requested");
                    let upload = next.clone();
                    stdin.close_async(
                        glib::Priority::DEFAULT,
                        Some(&next.cancellable),
                        move |result| {
                            if result.is_ok() {
                                upload.read_reply(Vec::new());
                            } else {
                                upload.finish(Err(upload.messages.finish.into()));
                            }
                        },
                    );
                }
                _ => next.finish(Err(next.messages.send.into())),
            },
        );
        Ok(upload)
    }

    fn read_reply(self: &Rc<Self>, mut reply: Vec<u8>) {
        let stdout = self.process.stdout_pipe().expect("stdout pipe requested");
        let next = self.clone();
        stdout.read_bytes_async(
            MAX_REPLY_BYTES + 1,
            glib::Priority::DEFAULT,
            Some(&self.cancellable),
            move |result| match result {
                Ok(bytes) if bytes.is_empty() => {
                    let upload = next.clone();
                    next.process
                        .wait_check_async(Some(&next.cancellable), move |result| {
                            let parsed = (upload.validate_reply)(&reply);
                            if upload.abandoned.get() {
                                if let Ok(path) = parsed {
                                    if let Some(cleanup) = upload.host_cleanup {
                                        cleanup(&path);
                                    }
                                }
                                upload.finish(Err(upload.messages.cancelled.into()));
                                return;
                            }
                            upload.finish(if result.is_ok() {
                                parsed
                            } else {
                                Err(upload.messages.failed.into())
                            });
                        });
                }
                Ok(bytes) if reply.len() + bytes.len() <= MAX_REPLY_BYTES => {
                    reply.extend_from_slice(&bytes);
                    next.read_reply(reply);
                }
                _ => next.finish(Err(next.messages.response.into())),
            },
        );
    }

    pub fn cancel(&self) {
        self.finish(Err(self.messages.cancelled.into()));
    }

    fn finish(&self, result: Result<String, String>) {
        if result.is_err() && self.host_cleanup.is_some() {
            // The reply read must keep running. The script disarms its own
            // cleanup after printing the path, so only that reply can name
            // the file a cancelled save has to delete.
            self.abandoned.set(true);
        }
        let Some(completion) = self.completion.borrow_mut().take() else {
            return;
        };
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.remove();
        }
        if result.is_err() {
            // SIGTERM is forwarded by host-spawn in Flatpak. Give SSH time to
            // close the remote stream before a hard fallback for stuck clients.
            if !self.exited.get() {
                self.process.send_signal(15);
                let process = self.process.clone();
                let exited = self.exited.clone();
                glib::timeout_add_local_once(Duration::from_secs(2), move || {
                    if !exited.get() {
                        process.force_exit();
                    }
                });
            }
            if self.host_cleanup.is_none() {
                self.cancellable.cancel();
            }
        }
        completion(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_arguments_preserve_alias_port_and_binary_transport() {
        let target = SshTarget::parse("alice@fe80::1%eth0", "2222").unwrap();
        let args = arguments(&target, false);
        assert_eq!(&args[..3], ["ssh", "-T", "-o"]);
        let boundary = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(&args[boundary - 2..boundary], ["-p", "2222"]);
        assert_eq!(args[boundary + 1], "alice@fe80::1%eth0");
        assert_eq!(args.len(), boundary + 3);
        assert!(args.contains(&"BatchMode=yes".into()));
        assert!(args.contains(&"StrictHostKeyChecking=yes".into()));
        assert!(args.contains(&"ClearAllForwardings=yes".into()));
        let host = arguments(&target, true);
        assert_eq!(
            &host[..4],
            [
                "/app/bin/host-spawn",
                "--no-pty",
                "--cwd=/",
                "--env=SSH_ASKPASS_REQUIRE"
            ]
        );
        assert_eq!(&host[4..], args);
    }

    #[test]
    fn host_stage_arguments_use_a_fixed_script_without_ssh_or_a_pty() {
        let args = host_stage_arguments();
        assert_eq!(
            &args[..5],
            [
                "/app/bin/host-spawn",
                "--no-pty",
                "--cwd=/",
                "/bin/sh",
                "-c"
            ]
        );
        assert_eq!(args.len(), 6);
        assert_eq!(args[5], UPLOAD_SCRIPT);
        assert!(args.iter().all(|arg| arg != "ssh" && arg != "--pty"));
    }

    #[test]
    fn clipboard_png_limit_rejects_a_byte_past_32_mib() {
        assert!(validate_clipboard_png(b"\x89PNG\r\n\x1a\nrest").is_ok());
        assert!(validate_clipboard_png(b"not a png").is_err());
        let mut oversized = vec![0_u8; MAX_PNG_BYTES + 1];
        oversized[..PNG_MAGIC.len()].copy_from_slice(PNG_MAGIC);
        assert!(validate_clipboard_png(&oversized).is_err());
        let mut boundary = vec![0_u8; MAX_PNG_BYTES];
        boundary[..PNG_MAGIC.len()].copy_from_slice(PNG_MAGIC);
        assert!(validate_clipboard_png(&boundary).is_ok());
    }

    #[test]
    fn only_private_upload_paths_are_accepted() {
        assert_eq!(
            remote_path(b"/tmp/limux-paste.aB01234567/image.png\n").unwrap(),
            "/tmp/limux-paste.aB01234567/image.png"
        );
        for value in [
            "",
            "/tmp/limux-paste.abcdefghij/image.png",
            "/etc/passwd\n",
            "/tmp/limux-paste.abc;echo x/image.png\n",
            "/tmp/limux-paste.../../evil/image.png\n",
            "/tmp/limux-paste.abcdefghij/image.png\ncommand\n",
        ] {
            assert!(remote_path(value.as_bytes()).is_err(), "{value}");
        }
    }

    fn run_upload(script: &str, timeout: Duration, cancel: bool) -> Result<String, String> {
        run_transfer(script, timeout, cancel, &SSH_TRANSFER, remote_path)
    }

    /// The default GLib context can be owned by only one thread.
    fn context_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap()
    }

    fn run_transfer(
        script: &str,
        timeout: Duration,
        cancel: bool,
        messages: &'static TransferMessages,
        validate: fn(&[u8]) -> Result<String, String>,
    ) -> Result<String, String> {
        let _lock = context_lock();
        let context = glib::MainContext::default();
        let _guard = context.acquire().unwrap();
        let results = Rc::new(RefCell::new(Vec::new()));
        let output = results.clone();
        let upload = Upload::spawn_with_reply(
            &["/bin/sh".into(), "-c".into(), script.into()],
            glib::Bytes::from_static(b"fixture"),
            timeout,
            Box::new(move |result| output.borrow_mut().push(result)),
            validate,
            messages,
            None,
        )
        .unwrap();
        if cancel {
            upload.cancel();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while (results.borrow().is_empty() || !upload.exited.get())
            && std::time::Instant::now() < deadline
        {
            for _ in 0..100 {
                if !context.pending() {
                    break;
                }
                context.iteration(false);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(upload.exited.get(), "upload subprocess must be reaped");
        upload.cancel();
        assert_eq!(results.borrow().len(), 1, "complete exactly once");
        let result = results.borrow_mut().pop().unwrap();
        result
    }

    #[test]
    fn asynchronous_upload_checks_exit_status_and_bounds_output() {
        let path = "/tmp/limux-paste.abcdefghij/image.png";
        assert_eq!(
            run_upload(
                &format!("cat >/dev/null; printf '%s\\n' '{path}'"),
                Duration::from_secs(2),
                false
            )
            .unwrap(),
            path
        );
        assert!(run_upload(
            &format!("cat >/dev/null; printf '%s\\n' '{path}'; exit 17"),
            Duration::from_secs(2),
            false
        )
        .is_err());
        assert!(run_upload(
            "cat >/dev/null; printf 'not-a-path\\n'",
            Duration::from_secs(2),
            false
        )
        .is_err());
        assert!(run_upload(
            "cat >/dev/null; head -c 4096 /dev/zero",
            Duration::from_secs(2),
            false
        )
        .is_err());
    }

    #[test]
    fn host_stage_transfer_reports_staging_failures() {
        let path = "/tmp/limux-paste.abcdefghij/image.png";
        assert_eq!(
            run_transfer(
                &format!("cat >/dev/null; printf '%s\\n' '{path}'"),
                Duration::from_secs(2),
                false,
                &HOST_STAGE,
                host_stage_path,
            )
            .unwrap(),
            path
        );
        assert!(run_transfer(
            "exec sleep 60",
            Duration::from_millis(40),
            false,
            &HOST_STAGE,
            host_stage_path,
        )
        .unwrap_err()
        .contains("timed out"));
        assert!(run_transfer(
            "exec sleep 60",
            Duration::from_secs(2),
            true,
            &HOST_STAGE,
            host_stage_path,
        )
        .unwrap_err()
        .contains("cancelled"));
        assert!(run_transfer(
            "cat >/dev/null; printf 'not-a-path\\n'",
            Duration::from_secs(2),
            false,
            &HOST_STAGE,
            host_stage_path,
        )
        .is_err());
        assert!(run_transfer(
            "cat >/dev/null; printf '\\n'",
            Duration::from_secs(2),
            false,
            &HOST_STAGE,
            host_stage_path,
        )
        .is_err());
        assert!(run_transfer(
            "cat >/dev/null; head -c 4096 /dev/zero",
            Duration::from_secs(2),
            false,
            &HOST_STAGE,
            host_stage_path,
        )
        .is_err());
        assert!(run_transfer(
            &format!("cat >/dev/null; printf '%s\\n' '{path}'; exit 17"),
            Duration::from_secs(2),
            false,
            &HOST_STAGE,
            host_stage_path,
        )
        .is_err());
    }

    #[test]
    fn cancelled_host_save_deletes_a_path_printed_before_delivery() {
        let script = r#"set -eu
umask 077
dir=$(mktemp -d /tmp/limux-paste.XXXXXXXXXX)
trap 'rm -f "$dir/image.png"; rmdir "$dir"' 0
trap 'exit 1' HUP INT TERM
cat > "$dir/image.png"
printf '%s\n' "$dir/image.png"
trap - 0 HUP INT TERM
exec sleep 30
"#;
        let _lock = context_lock();
        let context = glib::MainContext::default();
        let _guard = context.acquire().unwrap();
        let results = Rc::new(RefCell::new(Vec::new()));
        let output = results.clone();
        let before = std::fs::read_dir("/tmp")
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().starts_with("limux-paste."))
            .collect::<Vec<_>>();
        let upload = Upload::spawn_with_reply(
            &["/bin/sh".into(), "-c".into(), script.into()],
            glib::Bytes::from_static(b"fixture"),
            Duration::from_secs(5),
            Box::new(move |result| output.borrow_mut().push(result)),
            host_stage_path,
            &HOST_STAGE,
            Some(delete_local_host_paste),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let created = loop {
            let mut found = std::fs::read_dir("/tmp").unwrap().filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name();
                if before.iter().any(|existing| existing == &name) {
                    return None;
                }
                let name = name.to_string_lossy();
                name.starts_with("limux-paste.").then(|| entry.path())
            });
            if let Some(path) = found.next() {
                break path;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "host script did not create a paste directory"
            );
            for _ in 0..20 {
                if !context.pending() {
                    break;
                }
                context.iteration(false);
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        upload.cancel();
        let file = created.join("image.png");
        let gone = std::time::Instant::now() + Duration::from_secs(5);
        while created.exists() && std::time::Instant::now() < gone {
            for _ in 0..20 {
                if !context.pending() {
                    break;
                }
                context.iteration(false);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !created.exists() && !file.exists(),
            "cancelled host save left {}",
            created.display()
        );
        let result = results.borrow();
        assert_eq!(result.len(), 1);
        assert!(result[0].as_ref().unwrap_err().contains("cancelled"));
    }

    fn delete_local_host_paste(path: &str) {
        let Some(path) = validated_paste_path(path) else {
            return;
        };
        let directory = path.trim_end_matches("/image.png");
        let script = format!("rm -f -- '{path}'; rmdir -- '{directory}'");
        if let Ok(mut child) = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }

    #[test]
    fn upload_process_cannot_use_the_launching_terminal() {
        use std::ffi::{c_char, c_int, c_ulong, c_void};
        use std::io::Read;
        use std::os::fd::FromRawFd;
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        const CHILD_ENV: &str = "LIMUX_TEST_UPLOAD_CTTY_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            assert!(
                Command::new("/bin/sh")
                    .args(["-c", ": </dev/tty"])
                    .status()
                    .unwrap()
                    .success(),
                "the fixture must have a controlling terminal"
            );
            let script = r#"if ( : </dev/tty ) 2>/dev/null || test -t 2; then exit 17; fi
test "$(od -An -tx1 | tr -d ' \n')" = 66697874757265 || exit 18
printf '/tmp/limux-paste.abcdefghij/image.png\n'"#;
            assert_eq!(
                run_upload(script, Duration::from_secs(2), false).unwrap(),
                "/tmp/limux-paste.abcdefghij/image.png"
            );
            assert_eq!(
                run_transfer(
                    script,
                    Duration::from_secs(2),
                    false,
                    &HOST_STAGE,
                    host_stage_path,
                )
                .unwrap(),
                "/tmp/limux-paste.abcdefghij/image.png"
            );
            return;
        }

        #[link(name = "util")]
        unsafe extern "C" {
            fn openpty(
                master: *mut c_int,
                slave: *mut c_int,
                name: *mut c_char,
                termios: *const c_void,
                winsize: *const c_void,
            ) -> c_int;
            fn setsid() -> c_int;
            fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
        }
        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            unsafe {
                openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        // openpty returns two owned descriptors on success.
        let (master, slave) = unsafe {
            (
                std::fs::File::from_raw_fd(master),
                std::fs::File::from_raw_fd(slave),
            )
        };
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "ssh_transfer::tests::upload_process_cannot_use_the_launching_terminal",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        // The fixture, unlike the uploader, deliberately acquires a terminal.
        unsafe {
            command.pre_exec(|| {
                if setsid() < 0 || ioctl(0, 0x540e, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        drop(command);
        let output = std::thread::spawn(move || {
            let mut output = Vec::new();
            // Linux returns EIO after the last slave descriptor closes.
            let _ = master.take(64 * 1024).read_to_end(&mut output);
            output
        });
        let status = child.wait().unwrap();
        let output = output.join().unwrap();
        assert!(status.success(), "{}", String::from_utf8_lossy(&output));
    }

    #[test]
    fn stalled_and_cancelled_uploads_terminate_without_injection() {
        assert!(
            run_upload("exec sleep 60", Duration::from_millis(40), false)
                .unwrap_err()
                .contains("timed out")
        );
        assert!(run_upload("exec sleep 60", Duration::from_secs(2), true).is_err());
    }

    #[test]
    fn remote_script_creates_private_binary_file() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Command, Stdio};
        let args = arguments(&SshTarget::parse("host", "").unwrap(), false);
        let mut child = Command::new("/bin/sh")
            .args(["-c", args.last().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let bytes = b"\x89PNG\r\n\x1a\n\0\xffbinary\r\n";
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(result.status.success());
        let path = std::path::PathBuf::from(remote_path(&result.stdout).unwrap());
        let directory = path.parent().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn remote_script_removes_incomplete_uploads() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let marker = dir.path().join("allocated");
        let mktemp = bin.join("mktemp");
        std::fs::write(&mktemp, format!("#!/bin/sh\ndir=$(/usr/bin/mktemp -d '{}/upload.XXXXXXXXXX')\nprintf '%s' \"$dir\" > '{}'\nprintf '%s\\n' \"$dir\"\n", dir.path().display(), marker.display())).unwrap();
        let cat = bin.join("cat");
        std::fs::write(&cat, "#!/bin/sh\nexit 17\n").unwrap();
        for script in [&mktemp, &cat] {
            std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let result = std::process::Command::new("/bin/sh")
            .args(["-c", UPLOAD_SCRIPT])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!std::path::Path::new(&std::fs::read_to_string(marker).unwrap()).exists());
    }
}
