//! Binary image upload through OpenSSH. No local shell, staging file, or helper.
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
        format!("/bin/sh -c '{}'", UPLOAD_SCRIPT.replace('\'', "'\\''")),
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

fn remote_path(reply: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(reply).map_err(|_| "Invalid SSH upload response")?;
    let path = text
        .strip_suffix('\n')
        .ok_or("Invalid SSH upload response")?;
    let token = path
        .strip_prefix("/tmp/limux-paste.")
        .and_then(|value| value.strip_suffix("/image.png"))
        .filter(|value| value.len() == 10 && value.bytes().all(|c| c.is_ascii_alphanumeric()))
        .ok_or("Invalid SSH upload response")?;
    Ok(format!("/tmp/limux-paste.{token}/image.png"))
}

type Completion = Box<dyn FnOnce(Result<String, String>)>;
pub struct Upload {
    process: gio::Subprocess,
    cancellable: gio::Cancellable,
    completion: RefCell<Option<Completion>>,
    timer: RefCell<Option<glib::SourceId>>,
    exited: Rc<Cell<bool>>,
}

impl Upload {
    pub fn start(
        target: &SshTarget,
        png: glib::Bytes,
        flatpak: bool,
        completion: impl FnOnce(Result<String, String>) + 'static,
    ) -> Result<Rc<Self>, String> {
        if png.len() > MAX_PNG_BYTES || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("Clipboard image is invalid or exceeds 32 MiB".into());
        }
        let args = arguments(target, flatpak);
        Self::spawn(&args, png, Duration::from_secs(30), Box::new(completion))
    }

    fn spawn(
        args: &[String],
        png: glib::Bytes,
        timeout: Duration,
        completion: Completion,
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
            .map_err(|err| format!("Could not start SSH upload: {err}"))?;
        // Flatpak HostCommand starts a new session but can acquire a terminal
        // from any standard descriptor. Piping stderr keeps host SSH detached.
        relay_diagnostics(&process.stderr_pipe().expect("stderr pipe requested"));
        let upload = Rc::new(Self {
            process,
            cancellable: gio::Cancellable::new(),
            completion: RefCell::new(Some(completion)),
            timer: RefCell::new(None),
            exited: Rc::new(Cell::new(false)),
        });
        let exited = upload.exited.clone();
        upload
            .process
            .wait_async(gio::Cancellable::NONE, move |_| exited.set(true));
        let weak = Rc::downgrade(&upload);
        *upload.timer.borrow_mut() = Some(glib::timeout_add_local_once(timeout, move || {
            if let Some(upload) = weak.upgrade() {
                upload.timer.borrow_mut().take();
                upload.finish(Err("SSH image upload timed out".into()));
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
                                upload.finish(Err("Could not finish SSH image upload".into()));
                            }
                        },
                    );
                }
                _ => next.finish(Err("Could not send image over SSH".into())),
            },
        );
        Ok(upload)
    }

    fn read_reply(self: &Rc<Self>, mut reply: Vec<u8>) {
        let stdout = self.process.stdout_pipe().expect("stdout pipe requested");
        let next = self.clone();
        stdout.read_bytes_async(MAX_REPLY_BYTES + 1, glib::Priority::DEFAULT, Some(&self.cancellable), move |result| {
            match result {
                Ok(bytes) if bytes.is_empty() => {
                    let upload = next.clone();
                    next.process.wait_check_async(Some(&next.cancellable), move |result| {
                        upload.finish(if result.is_ok() { remote_path(&reply) } else {
                            Err("SSH upload failed. Check keys/agent, known_hosts, and the launch log.".into())
                        });
                    });
                }
                Ok(bytes) if reply.len() + bytes.len() <= MAX_REPLY_BYTES => {
                    reply.extend_from_slice(&bytes);
                    next.read_reply(reply);
                }
                _ => next.finish(Err("Invalid or failed SSH upload response".into())),
            }
        });
    }

    pub fn cancel(&self) {
        self.finish(Err("Image upload cancelled".into()));
    }

    fn finish(&self, result: Result<String, String>) {
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
            self.cancellable.cancel();
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
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _lock = LOCK.lock().unwrap();
        let context = glib::MainContext::default();
        let _guard = context.acquire().unwrap();
        let results = Rc::new(RefCell::new(Vec::new()));
        let output = results.clone();
        let upload = Upload::spawn(
            &["/bin/sh".into(), "-c".into(), script.into()],
            glib::Bytes::from_static(b"fixture"),
            timeout,
            Box::new(move |result| output.borrow_mut().push(result)),
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
            assert_eq!(
                run_upload(
                    r#"if ( : </dev/tty ) 2>/dev/null || test -t 2; then exit 17; fi
test "$(od -An -tx1 | tr -d ' \n')" = 66697874757265 || exit 18
printf '/tmp/limux-paste.abcdefghij/image.png\n'"#,
                    Duration::from_secs(2),
                    false,
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
