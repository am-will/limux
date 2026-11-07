use super::BrowserSandbox;
use gtk4::glib;
use std::collections::HashSet;
use std::fs;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::{fs::MetadataExt, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use webkit6::prelude::*;

const PROBE: &str = "--browser-sandbox-probe";
const WEBVIEW_PROBE: &str = "--browser-webview-probe";
const TOKEN: &str = "LIMUX_BROWSER_PROBE_TOKEN";
const APPIMAGE_RUNTIME_ERROR: &str = "The AppImage sandboxed browser runtime could not be prepared. Check that the cache directory is writable and permits executable files, then restart Limux.";
const TIMEOUT: Duration = Duration::from_secs(15);
const PR_SET_CHILD_SUBREAPER: i32 = 36;
const PR_SET_DUMPABLE: i32 = 4;
const SIGKILL: i32 = 9;
const RLIMIT_CORE: u32 = 4;
const WNOHANG: i32 = 1;
const SYS_PIDFD_OPEN: std::ffi::c_long = 434;
const SYS_PIDFD_SEND_SIGNAL: std::ffi::c_long = 424;

#[repr(C)]
struct ResourceLimit {
    current: usize,
    maximum: usize,
}

unsafe extern "C" {
    fn prctl(option: i32, ...) -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn syscall(number: std::ffi::c_long, ...) -> std::ffi::c_long;
    fn setrlimit(resource: u32, limit: *const ResourceLimit) -> i32;
}

struct ProbeDirectory(PathBuf);

impl ProbeDirectory {
    fn new() -> Result<Self, String> {
        glib::mkdtemp(std::env::temp_dir().join("limux-browser-probe-XXXXXX"))
            .map(Self)
            .ok_or_else(|| "Could not create a private browser probe directory.".to_string())
    }
}

impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn own_command(webview: bool, directory: Option<&Path>) -> Result<Command, String> {
    let mut command = Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
    #[cfg(not(test))]
    command.arg(if webview { WEBVIEW_PROBE } else { PROBE });
    #[cfg(test)]
    command.args([
        "--exact",
        if webview {
            "browser_sandbox::probe::tests::webview_probe_child"
        } else {
            "browser_sandbox::probe::tests::supervisor_child"
        },
        "--ignored",
        "--nocapture",
    ]);
    if let Some(directory) = directory {
        command.env(TOKEN, directory);
    }
    Ok(command)
}

pub(super) fn check() -> Result<BrowserSandbox, String> {
    require_prepared_runtime()?;
    let output = own_command(false, None)?
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("Could not start the browser sandbox check: {error}"))?;
    if !output.status.success() {
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Browser sandbox unavailable. {}\nCheck the application's AppArmor/user-namespace policy and WebKit sandbox helpers.",
            diagnostic.trim()
        ));
    }
    Ok(BrowserSandbox {
        sandboxed: !bypass_requested(),
    })
}

pub(super) fn run_if_requested() -> Option<i32> {
    match std::env::args().nth(1).as_deref() {
        Some(PROBE) => Some(finish(supervise())),
        Some(WEBVIEW_PROBE) => Some(finish(webview_probe())),
        _ => None,
    }
}

fn finish(result: Result<(), String>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn bypass_requested() -> bool {
    std::env::var("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS").as_deref() == Ok("1")
}

fn require_prepared_runtime() -> Result<(), String> {
    if std::env::var_os("LIMUX_APPIMAGE_WEBKIT_UNAVAILABLE").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        return Err(APPIMAGE_RUNTIME_ERROR.to_string());
    }
    Ok(())
}

fn supervise() -> Result<(), String> {
    require_prepared_runtime()?;
    // Only this disposable supervisor adopts WebKit helpers, never the GUI host.
    if unsafe { prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        return Err(format!(
            "Could not supervise browser helpers: {}",
            std::io::Error::last_os_error()
        ));
    }
    let directory = ProbeDirectory::new()?;
    let log = fs::File::create(directory.0.join("stderr")).map_err(|error| error.to_string())?;
    let mut child = own_command(true, Some(&directory.0))?
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|error| format!("Could not launch WebKit check: {error}"))?;
    let result = wait_for_browser(&mut child, &directory.0);
    let cleanup = cleanup(&mut child, &directory.0);
    match result.and(cleanup) {
        Ok(()) => Ok(()),
        Err(error) => {
            let log = fs::read_to_string(directory.0.join("stderr")).unwrap_or_default();
            let tail = log
                .lines()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            Err(if tail.is_empty() {
                error
            } else {
                format!("{error}\n{tail}")
            })
        }
    }
}

fn wait_for_browser(child: &mut Child, directory: &Path) -> Result<(), String> {
    wait_for_readiness(child, directory, TIMEOUT)?;
    if bypass_requested() {
        Ok(())
    } else {
        verify_sandbox(directory)
    }
}

fn wait_for_readiness(
    child: &mut Child,
    directory: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(format!("WebKit startup check exited with {status}."));
        }
        if directory.join("ready").is_file() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "WebKit startup check timed out after {} seconds while waiting for its JavaScript callback.",
                timeout.as_secs_f64()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct ProbeProcess {
    pid: u32,
    pidfd: OwnedFd,
}

impl ProbeProcess {
    fn open(pid: u32) -> Option<Self> {
        // pidfd numbers are shared by the supported x86_64 and aarch64 Linux ABIs.
        let fd = unsafe { syscall(SYS_PIDFD_OPEN, pid, 0) };
        (fd >= 0).then(|| Self {
            pid,
            pidfd: unsafe { OwnedFd::from_raw_fd(fd as i32) },
        })
    }

    fn kill(&self) {
        unsafe {
            syscall(
                SYS_PIDFD_SEND_SIGNAL,
                self.pidfd.as_raw_fd(),
                SIGKILL,
                std::ptr::null::<()>(),
                0,
            );
        }
    }
}

fn child_pids(pid: u32) -> Vec<u32> {
    let mut children = Vec::new();
    if let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) {
        for task in tasks.flatten() {
            if let Ok(pids) = fs::read_to_string(task.path().join("children")) {
                children.extend(
                    pids.split_whitespace()
                        .filter_map(|pid| pid.parse::<u32>().ok()),
                );
            }
        }
    }
    children.sort_unstable();
    children.dedup();
    children
}

fn probe_processes(directory: &Path) -> Result<Vec<ProbeProcess>, String> {
    let mut descendants = HashSet::new();
    let mut pending = child_pids(std::process::id());
    while let Some(pid) = pending.pop() {
        if descendants.insert(pid) {
            pending.extend(child_pids(pid));
        }
    }
    let token = format!("{TOKEN}={}", directory.display());
    let uid = fs::metadata("/proc/self")
        .map_err(|error| error.to_string())?
        .uid();
    let mut processes = Vec::new();
    for entry in fs::read_dir("/proc").map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let comm = fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        if !comm.starts_with("WebKit")
            && !comm.starts_with("bwrap")
            && !comm.starts_with("xdg-dbus-proxy")
            && !comm.starts_with("flatpak-spawn")
        {
            continue;
        }
        let same_user = fs::read_to_string(entry.path().join("status")).is_ok_and(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("Uid:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u32>()
                    .ok()
            }) == Some(uid)
        });
        if !same_user {
            continue;
        }
        let identity = fs::read_to_string(entry.path().join("stat")).unwrap_or_default();
        let Some(process) = ProbeProcess::open(pid) else {
            continue;
        };
        let owned = descendants.contains(&pid)
            || fs::read(entry.path().join("environ")).is_ok_and(|env| {
                env.split(|byte| *byte == 0)
                    .any(|value| value == token.as_bytes())
            });
        if owned
            && fs::read_to_string(entry.path().join("stat")).is_ok_and(|after| {
                process_start_time(&identity)
                    .is_some_and(|before| Some(before) == process_start_time(&after))
            })
        {
            processes.push(process);
        }
    }
    Ok(processes)
}

fn process_start_time(stat: &str) -> Option<&str> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)
}

fn verify_sandbox(directory: &Path) -> Result<(), String> {
    let own_namespace = fs::metadata("/proc/self/ns/user")
        .map_err(|error| error.to_string())?
        .ino();
    let mut found = false;
    for process in probe_processes(directory)? {
        let pid = process.pid;
        let comm = fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        if !comm.starts_with("WebKitWeb") {
            continue;
        }
        found = true;
        let status = fs::read_to_string(format!("/proc/{pid}/status"))
            .map_err(|error| format!("Cannot inspect WebKit sandbox: {error}"))?;
        let namespace = fs::metadata(format!("/proc/{pid}/ns/user"))
            .map_err(|error| format!("Cannot inspect WebKit user namespace: {error}"))?
            .ino();
        verify_status(&status, namespace != own_namespace)?;
    }
    if !found {
        return Err(
            "Could not verify the probe's WebKit process isolation through /proc.".to_string(),
        );
    }
    Ok(())
}

fn verify_status(status: &str, separate_namespace: bool) -> Result<(), String> {
    let field = |name| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name).map(str::trim))
    };
    if field("NoNewPrivs:") != Some("1")
        || field("Seccomp:") != Some("2")
        || field("CapEff:") != Some("0000000000000000")
        || !separate_namespace
    {
        return Err("WebKit started without the required seccomp, no-new-privileges, capability, and user-namespace isolation.".to_string());
    }
    Ok(())
}

fn cleanup(child: &mut Child, directory: &Path) -> Result<(), String> {
    if child
        .try_wait()
        .map_err(|error| error.to_string())?
        .is_none()
    {
        // The unreaped group leader still owns this process-group identity.
        unsafe {
            kill(-(child.id() as i32), SIGKILL);
        }
        let _ = child.kill();
    }
    let _ = child.wait();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let mut remaining = probe_processes(directory)?;
        remaining.extend(
            child_pids(std::process::id())
                .into_iter()
                .filter_map(ProbeProcess::open),
        );
        for process in &remaining {
            process.kill();
        }
        while unsafe { waitpid(-1, std::ptr::null_mut(), WNOHANG) } > 0 {}
        if remaining.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("Browser check helpers survived cleanup.".to_string());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn disable_core_dumps() -> Result<(), String> {
    // Namespace denial can make WebKit call g_error; an expected probe failure
    // must not create a core dump on every application launch. Pipe handlers
    // ignore RLIMIT_CORE, so also disable this disposable leaf's dumpability.
    if unsafe { prctl(PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        return Err(format!(
            "Could not disable browser probe core dumps: {}",
            std::io::Error::last_os_error()
        ));
    }
    let no_core_dump = ResourceLimit {
        current: 0,
        maximum: 0,
    };
    if unsafe { setrlimit(RLIMIT_CORE, &no_core_dump) } != 0 {
        return Err(format!(
            "Could not disable browser probe core dumps: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn webview_probe() -> Result<(), String> {
    require_prepared_runtime()?;
    disable_core_dumps()?;
    let directory = std::env::var_os(TOKEN)
        .map(PathBuf::from)
        .ok_or("Missing browser check directory.")?;
    eprintln!("WebKit sandbox check: initializing GTK.");
    gtk4::init().map_err(|error| error.to_string())?;
    eprintln!("WebKit sandbox check: GTK initialized; creating an ephemeral web view.");
    let content = webkit6::UserContentManager::new();
    content.register_script_message_handler("limuxSandboxProbe", None);
    content.connect_script_message_received(Some("limuxSandboxProbe"), move |_, value| {
        if value.to_str().as_str() == "ready" {
            if let Err(error) = fs::write(directory.join("ready"), b"ready") {
                eprintln!("WebKit sandbox check: could not record readiness: {error}");
                std::process::exit(1);
            }
            eprintln!("WebKit sandbox check: JavaScript callback received.");
        }
    });
    let view = webkit6::WebView::builder()
        .network_session(&webkit6::NetworkSession::new_ephemeral())
        .user_content_manager(&content)
        .build();
    view.connect_load_failed(|_, event, _, error| {
        eprintln!("WebKit sandbox check: local document load failed ({event:?}): {error}");
        std::process::exit(1);
    });
    view.connect_web_process_terminated(|_, reason| {
        eprintln!("WebKit sandbox check: web process terminated: {reason:?}");
        std::process::exit(1);
    });
    view.connect_load_changed(|_, event| {
        if event == webkit6::LoadEvent::Finished {
            eprintln!("WebKit sandbox check: local document finished loading.");
        }
    });
    eprintln!("WebKit sandbox check: loading the local JavaScript check.");
    view.load_html("<!doctype html><script>window.webkit.messageHandlers.limuxSandboxProbe.postMessage('ready')</script>", Some("about:blank"));
    glib::MainLoop::new(None, false).run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn unprepared_appimage_runtime_skips_browser_startup() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "browser_sandbox::probe::tests::unprepared_runtime_fixture_child",
                "--ignored",
                "--nocapture",
            ])
            .env("LIMUX_APPIMAGE_WEBKIT_UNAVAILABLE", "1")
            .env("TMPDIR", "/nonexistent/limux-browser-probe")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "isolated AppImage browser runtime preparation regression fixture"]
    fn unprepared_runtime_fixture_child() {
        assert_eq!(
            super::super::availability(),
            &Err(APPIMAGE_RUNTIME_ERROR.to_string())
        );
        assert_eq!(supervise(), Err(APPIMAGE_RUNTIME_ERROR.to_string()));
        assert_eq!(webview_probe(), Err(APPIMAGE_RUNTIME_ERROR.to_string()));
    }

    #[test]
    fn fatal_probe_signal_does_not_produce_a_core_dump() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "browser_sandbox::probe::tests::fatal_signal_fixture_child",
                "--ignored",
                "--nocapture",
            ])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.signal(), Some(5));
        assert!(!status.core_dumped());
    }

    #[test]
    #[ignore = "isolated browser probe coredump regression fixture"]
    fn fatal_signal_fixture_child() {
        disable_core_dumps().unwrap();
        unsafe {
            kill(std::process::id() as i32, 5);
        }
        panic!("SIGTRAP unexpectedly returned");
    }

    #[test]
    fn rejects_partial_isolation() {
        let status = "NoNewPrivs:\t1\nSeccomp:\t2\nCapEff:\t0000000000000000\n";
        assert!(verify_status(status, true).is_ok());
        assert!(verify_status(status, false).is_err());
        assert!(verify_status(&status.replace("Seccomp:\t2", "Seccomp:\t0"), true).is_err());
        assert!(verify_status(&status.replace("NoNewPrivs:\t1", "NoNewPrivs:\t0"), true).is_err());
        assert!(verify_status(
            &status.replace("CapEff:\t0000000000000000", "CapEff:\t0000000000000001"),
            true
        )
        .is_err());
    }

    #[test]
    fn timeout_cleans_up_detached_helpers() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "browser_sandbox::probe::tests::cleanup_fixture_child",
                "--ignored",
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "isolated child-subreaper regression fixture"]
    fn cleanup_fixture_child() {
        assert_eq!(unsafe { prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) }, 0);
        let directory = ProbeDirectory::new().unwrap();
        let mut child = Command::new("/bin/sh")
            .args(["-c", "setsid sleep 30 & wait"])
            .process_group(0)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while child_pids(child.id()).is_empty() {
            assert!(
                Instant::now() < deadline,
                "fixture did not create its detached helper"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let error =
            wait_for_readiness(&mut child, &directory.0, Duration::from_millis(100)).unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        cleanup(&mut child, &directory.0).unwrap();
        assert!(child_pids(std::process::id()).is_empty());
    }

    #[test]
    fn cold_start_readiness_can_exceed_five_seconds() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "browser_sandbox::probe::tests::delayed_readiness_fixture_child",
                "--ignored",
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "isolated cold-start readiness regression fixture"]
    fn delayed_readiness_fixture_child() {
        assert_eq!(unsafe { prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) }, 0);
        let directory = ProbeDirectory::new().unwrap();
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "sleep 6; printf ready > \"$1/ready\"; sleep 30",
                "probe",
            ])
            .arg(&directory.0)
            .process_group(0)
            .spawn()
            .unwrap();
        let result = wait_for_readiness(&mut child, &directory.0, TIMEOUT);
        let stopped = cleanup(&mut child, &directory.0);
        result.unwrap();
        stopped.unwrap();
        assert!(child_pids(std::process::id()).is_empty());
    }

    #[test]
    #[ignore = "internal browser probe subprocess"]
    fn supervisor_child() {
        std::process::exit(finish(supervise()));
    }

    #[test]
    #[ignore = "internal browser probe subprocess"]
    fn webview_probe_child() {
        std::process::exit(finish(webview_probe()));
    }
}
