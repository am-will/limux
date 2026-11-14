use super::BrowserSandbox;
use gtk4::glib;
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::{fs::MetadataExt, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use webkit6::prelude::*;

const PROBE: &str = "--browser-sandbox-probe";
const WEBVIEW_PROBE: &str = "--browser-webview-probe";
const TOKEN: &str = "LIMUX_BROWSER_PROBE_TOKEN";
const APPIMAGE_RUNTIME_ERROR: &str = "The AppImage sandboxed browser runtime could not be prepared. Check that the cache directory is writable and permits executable files, then restart Limux.";
const TIMEOUT: Duration = Duration::from_secs(15);
const HOST_CANCEL_TIMEOUT: Duration = Duration::from_secs(3);
const CANCELLATION_SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];
static TERMINATION_SIGNAL: AtomicI32 = AtomicI32::new(0);
static HOST_TERMINATION_SIGNAL: AtomicI32 = AtomicI32::new(0);
static HOST_SIGNAL_LOCK: Mutex<()> = Mutex::new(());

extern "C" fn request_cancellation(signal: libc::c_int) {
    TERMINATION_SIGNAL.store(signal, Ordering::Relaxed);
}

extern "C" fn record_host_signal(signal: libc::c_int) {
    let _ =
        HOST_TERMINATION_SIGNAL.compare_exchange(0, signal, Ordering::Relaxed, Ordering::Relaxed);
}

struct HostSignalGuard {
    saved: Vec<(libc::c_int, libc::sigaction)>,
}

impl HostSignalGuard {
    fn install() -> Result<Self, String> {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = record_host_signal as *const () as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
            for signal in CANCELLATION_SIGNALS {
                libc::sigaddset(&mut action.sa_mask, signal);
            }
        }
        let mut guard = Self {
            saved: Vec::with_capacity(CANCELLATION_SIGNALS.len()),
        };
        for signal in CANCELLATION_SIGNALS {
            let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
            if unsafe { libc::sigaction(signal, std::ptr::null(), &mut previous) } != 0 {
                return Err(format!(
                    "Could not inspect host signal disposition: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if previous.sa_sigaction == libc::SIG_IGN {
                continue;
            }
            if unsafe { libc::sigaction(signal, &action, &mut previous) } != 0 {
                return Err(format!(
                    "Could not handle host termination during browser check: {}",
                    std::io::Error::last_os_error()
                ));
            }
            guard.saved.push((signal, previous));
        }
        Ok(guard)
    }
}

impl Drop for HostSignalGuard {
    fn drop(&mut self) {
        for (signal, previous) in self.saved.drain(..).rev() {
            unsafe {
                libc::sigaction(signal, &previous, std::ptr::null_mut());
            }
        }
    }
}

fn reraise_host_signal(signal: libc::c_int) {
    unsafe {
        let mut mask: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut mask);
        libc::sigaddset(&mut mask, signal);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &mask, std::ptr::null_mut());
        libc::raise(signal);
    }
}

fn pause_briefly(duration: Duration) {
    let nanos = i32::try_from(duration.subsec_nanos()).unwrap_or(0);
    let delay = libc::timespec {
        tv_sec: libc::time_t::try_from(duration.as_secs()).unwrap_or(0),
        tv_nsec: nanos.into(),
    };
    unsafe {
        libc::nanosleep(&delay, std::ptr::null_mut());
    }
}

fn install_cancellation_handlers() -> Result<(), String> {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = request_cancellation as *const () as libc::sighandler_t;
    action.sa_flags = libc::SA_RESTART;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
        for signal in CANCELLATION_SIGNALS {
            libc::sigaddset(&mut action.sa_mask, signal);
        }
    }
    for signal in CANCELLATION_SIGNALS {
        if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0 {
            return Err(format!(
                "Could not handle browser check cancellation: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    // This disposable supervisor must receive cancellation even if its caller
    // blocked these signals. WebView children inherit the unblocked mask.
    let error =
        unsafe { libc::pthread_sigmask(libc::SIG_UNBLOCK, &action.sa_mask, std::ptr::null_mut()) };
    if error != 0 {
        return Err(format!(
            "Could not unblock browser check cancellation: {}",
            std::io::Error::from_raw_os_error(error)
        ));
    }
    check_cancellation()
}

fn check_cancellation() -> Result<(), String> {
    let signal = TERMINATION_SIGNAL.load(Ordering::Relaxed);
    if signal == 0 {
        Ok(())
    } else {
        Err(format!(
            "Browser startup check interrupted by signal {signal}."
        ))
    }
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

fn drain_stderr(stderr: &mut Option<ChildStderr>, output: &mut Vec<u8>) {
    let Some(pipe) = stderr.as_mut() else {
        return;
    };
    let mut chunk = [0_u8; 4096];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => {
                *stderr = None;
                break;
            }
            Ok(count) => output.extend_from_slice(&chunk[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => {
                *stderr = None;
                break;
            }
        }
    }
}

pub(super) fn check() -> Result<BrowserSandbox, String> {
    require_prepared_runtime()?;
    let signal_lock = HOST_SIGNAL_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    HOST_TERMINATION_SIGNAL.store(0, Ordering::SeqCst);
    let signal_guard = HostSignalGuard::install()?;
    let early_signal = HOST_TERMINATION_SIGNAL.load(Ordering::Relaxed);
    if early_signal != 0 {
        drop(signal_guard);
        drop(signal_lock);
        reraise_host_signal(early_signal);
        return Err(format!(
            "Browser startup check interrupted by signal {early_signal}."
        ));
    }

    let expected_ppid = unsafe { libc::getpid() };
    let mut command = own_command(false, None)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(move || {
            let mut default_action: libc::sigaction = std::mem::zeroed();
            default_action.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut default_action.sa_mask);
            for signal in CANCELLATION_SIGNALS {
                if libc::sigaction(signal, &default_action, std::ptr::null_mut()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            if libc::prctl(
                libc::PR_SET_PDEATHSIG,
                libc::SIGTERM as libc::c_ulong,
                libc::c_ulong::from(0_u8),
                libc::c_ulong::from(0_u8),
                libc::c_ulong::from(0_u8),
            ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != expected_ppid {
                libc::raise(libc::SIGTERM);
                return Err(std::io::Error::from_raw_os_error(libc::ECANCELED));
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start the browser sandbox check: {error}"))?;
    let mut stderr = child.stderr.take();
    if let Some(pipe) = &stderr {
        let fd = pipe.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags >= 0 {
            unsafe {
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
    }

    let child_pid = libc::pid_t::try_from(child.id())
        .map_err(|_| "Invalid browser sandbox check PID.".to_string())?;
    let mut stderr_bytes = Vec::new();
    let mut forwarded_at: Option<Instant> = None;
    let status = loop {
        drain_stderr(&mut stderr, &mut stderr_bytes);
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("Could not wait for the browser sandbox check: {error}"))?
        {
            drain_stderr(&mut stderr, &mut stderr_bytes);
            break status;
        }
        if HOST_TERMINATION_SIGNAL.load(Ordering::Relaxed) != 0 {
            match forwarded_at {
                None => {
                    unsafe {
                        libc::kill(child_pid, libc::SIGTERM);
                    }
                    forwarded_at = Some(Instant::now());
                }
                Some(started) if started.elapsed() >= HOST_CANCEL_TIMEOUT => {
                    let _ = child.kill();
                    let status = child.wait().map_err(|error| {
                        format!("Could not wait for the browser sandbox check: {error}")
                    })?;
                    drain_stderr(&mut stderr, &mut stderr_bytes);
                    break status;
                }
                Some(_) => {}
            }
        }
        if let Some(pipe) = &stderr {
            let mut pollfd = libc::pollfd {
                fd: pipe.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            unsafe {
                libc::poll(&mut pollfd, 1, 10);
            }
        } else {
            pause_briefly(Duration::from_millis(10));
        }
    };

    let caught_signal = HOST_TERMINATION_SIGNAL.load(Ordering::Relaxed);
    drop(signal_guard);
    drop(signal_lock);
    if caught_signal != 0 {
        reraise_host_signal(caught_signal);
        return Err(format!(
            "Browser startup check interrupted by signal {caught_signal}."
        ));
    }
    if !status.success() {
        let diagnostic = String::from_utf8_lossy(&stderr_bytes);
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

fn read_stderr_tail(path: &Path) -> String {
    let log = fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    log.lines()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

fn supervise() -> Result<(), String> {
    require_prepared_runtime()?;
    install_cancellation_handlers()?;
    // Only this disposable supervisor adopts WebKit helpers, never the GUI host.
    if unsafe {
        libc::prctl(
            libc::PR_SET_CHILD_SUBREAPER,
            libc::c_ulong::from(1_u8),
            libc::c_ulong::from(0_u8),
            libc::c_ulong::from(0_u8),
            libc::c_ulong::from(0_u8),
        )
    } != 0
    {
        return Err(format!(
            "Could not supervise browser helpers: {}",
            std::io::Error::last_os_error()
        ));
    }
    let directory = ProbeDirectory::new()?;
    check_cancellation()?;
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
    match result.and(cleanup).and_then(|()| check_cancellation()) {
        Ok(()) => Ok(()),
        Err(error) => {
            let tail = read_stderr_tail(&directory.0.join("stderr"));
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
        check_cancellation()?;
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
        pause_briefly(Duration::from_millis(20));
    }
}

struct ProbeProcess {
    pid: u32,
    pidfd: OwnedFd,
}

impl ProbeProcess {
    fn open(pid: u32) -> Option<Self> {
        let raw_pid = libc::pid_t::try_from(pid).ok()?;
        let fd = unsafe {
            libc::syscall(
                libc::SYS_pidfd_open,
                libc::c_long::from(raw_pid),
                libc::c_long::from(0_u8),
            )
        };
        let raw_fd = libc::c_int::try_from(fd).ok().filter(|&fd| fd >= 0)?;
        Some(Self {
            pid,
            pidfd: unsafe { OwnedFd::from_raw_fd(raw_fd) },
        })
    }

    fn kill(&self) {
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                libc::c_long::from(self.pidfd.as_raw_fd()),
                libc::c_long::from(libc::SIGKILL),
                std::ptr::null::<libc::siginfo_t>(),
                libc::c_long::from(0_u8),
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
        if let Ok(child_pid) = libc::pid_t::try_from(child.id()) {
            // The unreaped group leader still owns this process-group identity.
            unsafe {
                libc::kill(-child_pid, libc::SIGKILL);
            }
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
        while unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) } > 0 {}
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
    if unsafe {
        libc::prctl(
            libc::PR_SET_DUMPABLE,
            libc::c_ulong::from(0_u8),
            libc::c_ulong::from(0_u8),
            libc::c_ulong::from(0_u8),
            libc::c_ulong::from(0_u8),
        )
    } != 0
    {
        return Err(format!(
            "Could not disable browser probe core dumps: {}",
            std::io::Error::last_os_error()
        ));
    }
    let no_core_dump = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &no_core_dump) } != 0 {
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

    fn enable_subreaper() {
        assert_eq!(
            unsafe {
                libc::prctl(
                    libc::PR_SET_CHILD_SUBREAPER,
                    libc::c_ulong::from(1_u8),
                    libc::c_ulong::from(0_u8),
                    libc::c_ulong::from(0_u8),
                    libc::c_ulong::from(0_u8),
                )
            },
            0
        );
    }

    #[test]
    fn interrupted_supervisor_cleans_up_detached_helpers() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "browser_sandbox::probe::tests::interrupted_supervisor_fixture_child",
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
    #[ignore = "isolated supervisor interruption regression fixture"]
    fn interrupted_supervisor_fixture_child() {
        enable_subreaper();
        struct Fixture {
            child: Child,
            directory: tempfile::TempDir,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = cleanup(&mut self.child, self.directory.path());
            }
        }
        for (signal, blocked) in [
            (libc::SIGINT, false),
            (libc::SIGTERM, false),
            (libc::SIGTERM, true),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let report = directory.path().join("fixture-ready");
            let mut command = own_command(false, None).unwrap();
            command
                .env("TMPDIR", directory.path())
                .env("LIMUX_TEST_PROBE_INTERRUPT", &report)
                .env_remove("LIMUX_APPIMAGE_WEBKIT_UNAVAILABLE")
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .process_group(0);
            if blocked {
                unsafe {
                    command.pre_exec(|| {
                        let mut mask = std::mem::zeroed();
                        libc::sigemptyset(&mut mask);
                        libc::sigaddset(&mut mask, libc::SIGTERM);
                        let error =
                            libc::pthread_sigmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut());
                        if error == 0 {
                            Ok(())
                        } else {
                            Err(std::io::Error::from_raw_os_error(error))
                        }
                    });
                }
            }
            let mut fixture = Fixture {
                child: command.spawn().unwrap(),
                directory,
            };
            let deadline = Instant::now() + Duration::from_secs(5);
            while !report.is_file() {
                assert!(Instant::now() < deadline, "fixture did not become ready");
                assert!(fixture.child.try_wait().unwrap().is_none());
                std::thread::sleep(Duration::from_millis(10));
            }
            let report = fs::read_to_string(report).unwrap();
            let mut lines = report.lines();
            let probe_directory = PathBuf::from(lines.next().unwrap());
            let supervisor_pid = lines.next().unwrap().parse::<u32>().unwrap();
            assert_eq!(supervisor_pid, fixture.child.id());
            let pids = lines
                .map(|pid| pid.parse::<u32>().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(pids.len(), 3);
            for pid in &pids {
                assert!(Path::new(&format!("/proc/{pid}")).exists());
            }
            let child_pid = libc::pid_t::try_from(fixture.child.id()).unwrap();
            let target = if signal == libc::SIGINT {
                -child_pid
            } else {
                child_pid
            };
            assert_eq!(unsafe { libc::kill(target, signal) }, 0);
            let deadline = Instant::now() + Duration::from_secs(3);
            let status = loop {
                if let Some(status) = fixture.child.try_wait().unwrap() {
                    break status;
                }
                assert!(
                    Instant::now() < deadline,
                    "interrupted supervisor did not exit"
                );
                // Repeated cancellation must leave the cleanup path running.
                unsafe {
                    libc::kill(target, signal);
                }
                std::thread::sleep(Duration::from_millis(1));
            };
            assert!(
                !probe_directory.exists(),
                "probe directory survived cancellation"
            );
            for pid in pids {
                assert!(
                    !Path::new(&format!("/proc/{pid}")).exists(),
                    "helper {pid} survived cancellation"
                );
            }
            assert_eq!(status.code(), Some(1));
            let mut diagnostic = String::new();
            fixture
                .child
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut diagnostic)
                .unwrap();
            assert!(diagnostic.contains("interrupted"), "{diagnostic}");
        }
    }

    #[test]
    fn terminated_host_cleans_up_probe_supervisor_and_helpers() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "browser_sandbox::probe::tests::terminated_host_fixture_child",
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
    #[ignore = "isolated host termination propagation regression fixture"]
    fn terminated_host_fixture_child() {
        enable_subreaper();
        struct HostFixture {
            child: Child,
            directory: tempfile::TempDir,
        }
        impl Drop for HostFixture {
            fn drop(&mut self) {
                let _ = cleanup(&mut self.child, self.directory.path());
            }
        }

        for (signal, group_target) in [
            (libc::SIGTERM, false),
            (libc::SIGINT, false),
            (libc::SIGINT, true),
            (libc::SIGHUP, false),
            (libc::SIGKILL, false),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let report = directory.path().join("fixture-ready");
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "browser_sandbox::probe::tests::host_probe_check_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("TMPDIR", directory.path())
                .env("LIMUX_TEST_PROBE_INTERRUPT", &report)
                .env_remove("LIMUX_APPIMAGE_WEBKIT_UNAVAILABLE")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0);
            let mut fixture = HostFixture {
                child: command.spawn().unwrap(),
                directory,
            };
            let deadline = Instant::now() + Duration::from_secs(5);
            while !report.is_file() {
                assert!(Instant::now() < deadline, "host probe fixture not ready");
                assert!(fixture.child.try_wait().unwrap().is_none());
                std::thread::sleep(Duration::from_millis(10));
            }
            let report_text = fs::read_to_string(&report).unwrap();
            let mut lines = report_text.lines();
            let probe_directory = PathBuf::from(lines.next().unwrap());
            let pids = lines
                .map(|pid| pid.parse::<u32>().unwrap())
                .collect::<Vec<_>>();
            // [P2 supervisor, P3 webview_probe, detached setsid helper, token-matched WebKitWebProcess]
            assert_eq!(pids.len(), 4);
            assert!(probe_directory.is_dir());
            assert!(probe_directory
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("limux-browser-probe-"));
            let webkit_comm = fs::read_to_string(format!("/proc/{}/comm", pids[3])).unwrap();
            assert!(webkit_comm.starts_with("WebKit"), "{webkit_comm}");
            for pid in &pids {
                assert!(Path::new(&format!("/proc/{pid}")).exists());
            }

            let host_pid = libc::pid_t::try_from(fixture.child.id()).unwrap();
            let target = if group_target { -host_pid } else { host_pid };
            assert_eq!(unsafe { libc::kill(target, signal) }, 0);

            let wait_deadline = Instant::now() + Duration::from_secs(5);
            let status = loop {
                if let Some(status) = fixture.child.try_wait().unwrap() {
                    break status;
                }
                assert!(
                    Instant::now() < wait_deadline,
                    "host did not exit after signal {signal}"
                );
                std::thread::sleep(Duration::from_millis(5));
            };
            assert_eq!(status.signal(), Some(signal));

            if signal == libc::SIGKILL {
                let pdeath_deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    while unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) } > 0 {}
                    if !probe_directory.exists()
                        && pids
                            .iter()
                            .all(|pid| !Path::new(&format!("/proc/{pid}")).exists())
                    {
                        break;
                    }
                    assert!(
                        Instant::now() < pdeath_deadline,
                        "PR_SET_PDEATHSIG did not clean up probe supervisor and helpers after SIGKILL"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            } else {
                // Catchable host termination MUST reap P2, P3, helpers, and probe dir BEFORE P1 exits.
                assert!(
                    !probe_directory.exists(),
                    "probe directory {} still existed when P1 exited from signal {signal}",
                    probe_directory.display()
                );
                for pid in &pids {
                    assert!(
                        !Path::new(&format!("/proc/{pid}")).exists(),
                        "process {pid} still existed when P1 exited from signal {signal}"
                    );
                }
            }
            assert!(child_pids(std::process::id()).is_empty());
        }
    }

    #[test]
    fn host_check_restores_signal_dispositions() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "browser_sandbox::probe::tests::signal_restore_fixture_child",
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

    extern "C" fn custom_noop_handler(_: libc::c_int) {}

    #[test]
    #[ignore = "isolated host signal disposition restore fixture"]
    fn signal_restore_fixture_child() {
        unsafe {
            let mut ignore: libc::sigaction = std::mem::zeroed();
            ignore.sa_sigaction = libc::SIG_IGN;
            assert_eq!(
                libc::sigaction(libc::SIGHUP, &ignore, std::ptr::null_mut()),
                0
            );
            let mut custom: libc::sigaction = std::mem::zeroed();
            custom.sa_sigaction = custom_noop_handler as *const () as libc::sighandler_t;
            assert_eq!(
                libc::sigaction(libc::SIGTERM, &custom, std::ptr::null_mut()),
                0
            );
        }
        std::env::set_var("LIMUX_TEST_PROBE_EXIT_IMMEDIATELY", "1");
        std::env::remove_var("LIMUX_APPIMAGE_WEBKIT_UNAVAILABLE");
        let result = check();
        assert!(result.is_err());

        unsafe {
            let mut after_hup: libc::sigaction = std::mem::zeroed();
            assert_eq!(
                libc::sigaction(libc::SIGHUP, std::ptr::null(), &mut after_hup),
                0
            );
            assert_eq!(after_hup.sa_sigaction, libc::SIG_IGN);

            let mut after_term: libc::sigaction = std::mem::zeroed();
            assert_eq!(
                libc::sigaction(libc::SIGTERM, std::ptr::null(), &mut after_term),
                0
            );
            assert_eq!(
                after_term.sa_sigaction,
                custom_noop_handler as *const () as libc::sighandler_t
            );
        }
    }

    #[test]
    fn read_stderr_tail_preserves_non_utf8_diagnostics() {
        let directory = tempfile::tempdir().unwrap();
        let log_path = directory.path().join("stderr");
        assert_eq!(read_stderr_tail(&log_path), "");

        let mut raw = Vec::new();
        for index in 1..=14 {
            raw.extend_from_slice(format!("line {index}\n").as_bytes());
        }
        raw.extend_from_slice(b"invalid utf8: \xff\xfe end\n");
        fs::write(&log_path, &raw).unwrap();

        let tail = read_stderr_tail(&log_path);
        let lines = tail.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 12);
        assert_eq!(lines[0], "line 4");
        assert!(lines[11].starts_with("invalid utf8: \u{fffd}"));
        assert!(lines[11].ends_with(" end"));
    }

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
        assert_eq!(status.signal(), Some(libc::SIGTRAP));
        assert!(!status.core_dumped());
    }

    #[test]
    #[ignore = "isolated browser probe coredump regression fixture"]
    fn fatal_signal_fixture_child() {
        disable_core_dumps().unwrap();
        unsafe {
            libc::kill(
                libc::pid_t::try_from(std::process::id()).unwrap(),
                libc::SIGTRAP,
            );
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
        enable_subreaper();
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
        enable_subreaper();
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
    #[ignore = "internal browser host check subprocess"]
    fn host_probe_check_child() {
        let code = finish(check().map(|_| ()));
        std::process::exit(code);
    }

    #[test]
    #[ignore = "internal browser probe subprocess"]
    fn supervisor_child() {
        if std::env::var_os("LIMUX_TEST_PROBE_EXIT_IMMEDIATELY").is_some() {
            std::process::exit(1);
        }
        std::process::exit(finish(supervise()));
    }

    #[test]
    #[ignore = "internal fake WebKit helper subprocess"]
    fn webkit_helper_child() {
        let pid_file =
            PathBuf::from(std::env::var_os("LIMUX_TEST_WEBKIT_HELPER_PID_FILE").unwrap());
        let tmp = pid_file.with_extension("tmp");
        fs::write(&tmp, std::process::id().to_string()).unwrap();
        fs::rename(tmp, pid_file).unwrap();
        std::thread::sleep(Duration::from_secs(30));
        std::process::exit(0);
    }

    #[test]
    #[ignore = "internal browser probe subprocess"]
    fn webview_probe_child() {
        if let Some(report) = std::env::var_os("LIMUX_TEST_PROBE_INTERRUPT") {
            let directory = PathBuf::from(std::env::var_os(TOKEN).unwrap());
            let mut helper = Command::new("setsid")
                .args([
                    "/bin/sh",
                    "-c",
                    "printf '%s' \"$$\" > \"$1/helper\"; exec sleep 30",
                    "probe",
                ])
                .arg(&directory)
                .spawn()
                .unwrap();
            let report = PathBuf::from(report);
            let webkit_binary = report.with_file_name("WebKitWebProcess");
            let _ = fs::remove_file(&webkit_binary);
            std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &webkit_binary).unwrap();
            let webkit_pid_file = directory.join("webkit-helper");
            let status = Command::new("/bin/sh")
                .args([
                    "-c",
                    "setsid \"$1\" --exact browser_sandbox::probe::tests::webkit_helper_child --ignored --nocapture >/dev/null 2>&1 < /dev/null &",
                    "probe",
                ])
                .arg(&webkit_binary)
                .env("LIMUX_TEST_WEBKIT_HELPER_PID_FILE", &webkit_pid_file)
                .status()
                .unwrap();
            assert!(status.success());
            let deadline = Instant::now() + Duration::from_secs(3);
            let (helper_pid, webkit_pid) = loop {
                let helper_pid = fs::read_to_string(directory.join("helper"))
                    .ok()
                    .and_then(|pid| pid.parse::<u32>().ok());
                let webkit_pid = fs::read_to_string(&webkit_pid_file)
                    .ok()
                    .and_then(|pid| pid.parse::<u32>().ok());
                if let (Some(helper_pid), Some(webkit_pid)) = (helper_pid, webkit_pid) {
                    break (helper_pid, webkit_pid);
                }
                assert!(
                    Instant::now() < deadline,
                    "detached helpers did not become ready"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            let temporary_report = report.with_extension("tmp");
            fs::write(
                &temporary_report,
                format!(
                    "{}\n{}\n{}\n{helper_pid}\n{webkit_pid}\n",
                    directory.display(),
                    unsafe { libc::getppid() },
                    std::process::id(),
                ),
            )
            .unwrap();
            fs::rename(temporary_report, report).unwrap();
            let _ = helper.wait();
            std::process::exit(0);
        }
        std::process::exit(finish(webview_probe()));
    }
}
