#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Exercise an extracted package without rebuilding or installing its binaries."""

import argparse
import contextlib
import ctypes
import http.server
import json
import os
import re
import secrets
import select
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path


def require_pidfd_support():
    missing = [
        f"{module.__name__}.{name}"
        for module, name in (
            (os, "pidfd_open"),
            (os, "P_PIDFD"),
            (os, "waitid"),
            (signal, "pidfd_send_signal"),
        )
        if getattr(module, name, None) is None
    ]
    if missing:
        raise RuntimeError(
            f"Python at {sys.executable} lacks Linux pidfd support: {', '.join(missing)}. "
            "Select a Python build with these APIs, for example "
            "UV_PYTHON=/usr/bin/python3 on Ubuntu 24.04."
        )


def wait_for(check, processes, description):
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        for process in processes:
            if process.poll() is not None:
                raise RuntimeError(
                    f"{description}: child exited with {process.returncode}"
                )
        result = check()
        if result:
            return result
        time.sleep(0.2)
    raise RuntimeError(f"timed out waiting for {description}")


def child_pids(pid):
    children = set()
    for path in Path(f"/proc/{pid}/task").glob("*/children"):
        with contextlib.suppress(FileNotFoundError, ProcessLookupError):
            children.update(int(value) for value in path.read_text().split())
    return children


def descendants(root_pid):
    pending = [root_pid]
    seen = {root_pid}
    while pending:
        pid = pending.pop()
        try:
            children = child_pids(pid)
        except PermissionError as error:
            raise RuntimeError(
                f"cannot enumerate child processes below PID {pid}; "
                f"WebKit sandbox verification is blocked by /proc permissions: {error}"
            ) from error
        for child in children - seen:
            seen.add(child)
            pending.append(child)
    return seen - {root_pid}


def verify_webkit_sandbox(host_pid):
    limux_hosts = []
    web_processes = []
    for pid in descendants(host_pid):
        try:
            executable = Path(os.readlink(f"/proc/{pid}/exe")).name
        except (FileNotFoundError, ProcessLookupError):
            continue
        except PermissionError:
            try:
                comm = Path(f"/proc/{pid}/comm").read_text().strip()
            except (FileNotFoundError, ProcessLookupError):
                continue
            except PermissionError as error:
                raise RuntimeError(
                    f"cannot identify child PID {pid}; WebKit sandbox verification "
                    f"is blocked by /proc permissions: {error}"
                ) from error
            if "WebKitWeb" in comm:
                web_processes.append(pid)
            continue
        if Path(executable).name in {"limux", "limux-host"}:
            limux_hosts.append(pid)
        elif "WebKitWebProcess" in executable:
            web_processes.append(pid)
    if not limux_hosts:
        raise RuntimeError(
            f"could not locate the Limux host below launcher PID {host_pid}; "
            "its namespace could not be used as the WebKit sandbox baseline"
        )
    if not web_processes:
        raise RuntimeError(
            "could not locate the WebKitWebProcess below the packaged host; "
            "its sandbox state could not be verified"
        )

    host_user_namespace = os.stat(f"/proc/{limux_hosts[0]}/ns/user").st_ino
    inspected = False
    for pid in web_processes:
        try:
            status = Path(f"/proc/{pid}/status").read_text()
            user_namespace = os.stat(f"/proc/{pid}/ns/user").st_ino
        except (FileNotFoundError, ProcessLookupError):
            continue
        except PermissionError as error:
            raise RuntimeError(
                f"cannot inspect WebKitWebProcess {pid} sandbox state through /proc: {error}"
            ) from error
        inspected = True
        fields = {
            line.split(":", 1)[0]: line.split(":", 1)[1].strip()
            for line in status.splitlines()
            if ":" in line
        }
        sandbox = {
            "NoNewPrivs": fields.get("NoNewPrivs"),
            "Seccomp": fields.get("Seccomp"),
            "CapEff": fields.get("CapEff"),
            "separate_user_namespace": user_namespace != host_user_namespace,
        }
        if (
            sandbox["NoNewPrivs"] != "1"
            or sandbox["Seccomp"] != "2"
            or sandbox["CapEff"] != "0000000000000000"
            or not sandbox["separate_user_namespace"]
        ):
            raise RuntimeError(
                f"WebKitWebProcess {pid} did not enter the default sandbox: {sandbox}"
            )
        print(f"WebKitWebProcess {pid} sandbox: {sandbox}", flush=True)
    if not inspected:
        raise RuntimeError("WebKitWebProcess exited before sandbox inspection")


def assert_no_browser_sandbox_processes(host_pid):
    limux_hosts = []
    for pid in descendants(host_pid):
        try:
            name = Path(os.readlink(f"/proc/{pid}/exe")).name
        except (FileNotFoundError, ProcessLookupError):
            continue
        except PermissionError as error:
            raise RuntimeError(
                f"cannot check for leftover WebKit processes below PID {pid}: {error}"
            ) from error
        if name in {"limux", "limux-host"}:
            limux_hosts.append(pid)
        if "WebKit" in name or name in {"bwrap", "xdg-dbus-proxy"}:
            raise RuntimeError(
                f"browser sandbox helper {name} (PID {pid}) remains after rejection"
            )
    if len(limux_hosts) > 1:
        raise RuntimeError(
            "a disposable Limux sandbox-probe process remains after rejection: "
            f"{limux_hosts}"
        )


def compile_userns_denial_launcher(output):
    compiler = shutil.which("cc")
    if compiler is None:
        raise RuntimeError("--deny-userns requires a C compiler (cc)")
    fixture = Path(__file__).resolve().parent / "tests/deny-userns-launcher.c"
    result = subprocess.run(
        [compiler, "-O2", "-Wall", "-Wextra", "-Werror", str(fixture), "-o", str(output)],
        capture_output=True,
        text=True,
        check=False,
        timeout=30,
    )
    if result.returncode:
        raise RuntimeError(
            "could not build the child-local user-namespace denial launcher: "
            f"{result.stderr.strip()}"
        )
    baseline = subprocess.run(
        ["unshare", "-Ur", "true"],
        capture_output=True,
        text=True,
        check=False,
        timeout=5,
    )
    if baseline.returncode:
        raise RuntimeError(
            "--deny-userns requires unprivileged user namespaces to work before "
            "the child-local filter is installed: "
            f"{baseline.stderr.strip() or baseline.stdout.strip()}"
        )
    probe = subprocess.run(
        [str(output), "unshare", "-Ur", "true"],
        capture_output=True,
        text=True,
        check=False,
        timeout=5,
    )
    if probe.returncode == 0 or "operation not permitted" not in probe.stderr.lower():
        raise RuntimeError(
            "test launcher did not deny unshare(CLONE_NEWUSER) with EPERM: "
            f"exit={probe.returncode}, stderr={probe.stderr.strip()}"
        )
    print(
        "Child-local seccomp fixture changed unshare(CLONE_NEWUSER) from allowed to EPERM.",
        flush=True,
    )


def assert_apparmor_profile(host_pid, expected_profile):
    candidates = {host_pid, *descendants(host_pid)}
    for pid in candidates:
        try:
            executable = os.readlink(f"/proc/{pid}/exe")
        except (FileNotFoundError, ProcessLookupError):
            continue
        if Path(executable.removesuffix(" (deleted)")).name not in {
            "limux",
            "limux-host",
        }:
            continue
        try:
            profile = Path(f"/proc/{pid}/attr/current").read_text().strip()
        except (FileNotFoundError, ProcessLookupError, PermissionError) as error:
            raise RuntimeError(
                f"cannot read AppArmor profile for Limux host PID {pid}: {error}"
            ) from error
        if not profile.startswith(expected_profile) or (
            len(profile) > len(expected_profile)
            and not profile[len(expected_profile)].isspace()
        ):
            raise RuntimeError(
                f"Limux host PID {pid} has AppArmor label {profile!r}, "
                f"expected {expected_profile!r}"
            )
        print(f"Limux host PID {pid} AppArmor label: {profile}", flush=True)
        return
    raise RuntimeError(
        f"could not locate limux-host below launcher PID {host_pid} "
        "to verify its AppArmor profile"
    )


def find_browser_tabs(value):
    if isinstance(value, dict):
        if value.get("tab_kind") == "browser":
            yield value
        for child in value.values():
            yield from find_browser_tabs(child)
    elif isinstance(value, list):
        for child in value:
            yield from find_browser_tabs(child)


def saved_browser_state(session_path):
    try:
        session = json.loads(session_path.read_text())
    except (OSError, ValueError) as error:
        raise RuntimeError(f"cannot read saved browser session: {error}") from error
    return sorted(
        (
            tab.get("id"),
            tab.get("custom_name"),
            tab.get("pinned"),
            tab.get("uri"),
        )
        for tab in find_browser_tabs(session)
    )


def stop_process_tree(process, timeout=5):
    with contextlib.ExitStack() as stack:
        owned = {}
        root_fd = os.pidfd_open(process.pid)
        stack.callback(os.close, root_fd)
        owned[process.pid] = os.dup(root_fd)

        def capture_and_signal(sig):
            if process.poll() is not None:
                return
            with contextlib.ExitStack() as snapshot:
                for pid, fd in capture_processes(process.pid, snapshot):
                    if pid in owned and not select.select([owned[pid]], [], [], 0)[0]:
                        continue
                    if pid in owned:
                        os.close(owned.pop(pid))
                    owned[pid] = os.dup(fd)
                    with contextlib.suppress(ProcessLookupError):
                        signal.pidfd_send_signal(owned[pid], sig)

        def reap():
            process.poll()
            for pid, fd in owned.items():
                if pid != process.pid:
                    with contextlib.suppress(ChildProcessError):
                        os.waitid(os.P_PIDFD, fd, os.WEXITED | os.WNOHANG)

        def exited():
            return len(select.select(list(owned.values()), [], [], 0)[0]) == len(owned)

        def signal_owned(sig):
            for fd in owned.values():
                with contextlib.suppress(ProcessLookupError):
                    signal.pidfd_send_signal(fd, sig)

        stack.callback(lambda: [os.close(fd) for fd in owned.values()])
        capture_and_signal(signal.SIGTERM)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            capture_and_signal(signal.SIGTERM)
            reap()
            if exited():
                return
            time.sleep(0.05)
        signal_owned(signal.SIGKILL)
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            raise RuntimeError(f"host process {process.pid} survived forced shutdown") from error
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            reap()
            if exited():
                return
            time.sleep(0.05)
        raise RuntimeError("host child processes survived forced shutdown")


@contextlib.contextmanager
def child_subreaper():
    libc = ctypes.CDLL(None, use_errno=True)
    previous = ctypes.c_int()
    if libc.prctl(37, ctypes.byref(previous), 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "PR_GET_CHILD_SUBREAPER failed")
    if libc.prctl(36, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "PR_SET_CHILD_SUBREAPER failed")
    try:
        yield
    finally:
        if libc.prctl(36, previous.value, 0, 0, 0) != 0:
            raise OSError(ctypes.get_errno(), "restoring child subreaper failed")


def capture_processes(root_pid, stack):
    fd = os.pidfd_open(root_pid)
    stack.callback(os.close, fd)
    owned = [(root_pid, fd)]
    for pid, parent_fd in owned:
        for child in child_pids(pid):
            with contextlib.suppress(FileNotFoundError, ProcessLookupError):
                fd = os.pidfd_open(child)
                stack.callback(os.close, fd)
                parent = int(
                    Path(f"/proc/{child}/stat").read_text().rsplit(")", 1)[1].split()[1]
                )
                # Direct children cannot be recycled until this harness reaps
                # them. Retain their zombies too, so adopted children get reaped.
                if (
                    parent == pid
                    and not select.select([parent_fd], [], [], 0)[0]
                    and (pid == os.getpid() or not select.select([fd], [], [], 0)[0])
                ):
                    owned.append((child, fd))
    return owned[1:]


def mounts_under(root):
    mounts = []
    for line in Path("/proc/self/mountinfo").read_text().splitlines():
        fields, filesystem = line.split(" - ", 1)
        target = Path(
            re.sub(
                r"\\([0-7]{3})", lambda match: chr(int(match[1], 8)), fields.split()[4]
            )
        )
        if target.is_relative_to(root):
            kind, source, options = filesystem.split()
            mounts.append((target, kind, source, options.split(",")))
    return mounts


def detach_runtime_mounts(runtime):
    allowed = {
        runtime / "doc": ("fuse.portal", "portal"),
        runtime / "gvfs": ("fuse.gvfsd-fuse", "gvfsd-fuse"),
    }
    mounts = mounts_under(runtime)
    for target, kind, source, options in mounts:
        if (
            allowed.get(target) != (kind, source)
            or f"user_id={os.getuid()}" not in options
        ):
            raise RuntimeError(
                f"refusing to unmount unexpected private mount: {target}"
            )
    for target, _kind, _source, _options in mounts:
        print(f"Detaching lingering private runtime mount: {target}", flush=True)
        try:
            subprocess.run(
                ["fusermount3", "-u", "-z", "--", str(target)],
                check=True,
                timeout=5,
            )
        except subprocess.CalledProcessError:
            # Auto-unmount may win the race after the mountinfo snapshot.
            if any(mount[0] == target for mount in mounts_under(runtime)):
                raise


def remove_runtime_directory(root):
    # Never traverse a FUSE mount, including a disconnected one. stat() on a
    # stale document-portal mount raises ENOTCONN instead of identifying it.
    mounts = mounts_under(root)
    if mounts:
        raise RuntimeError(f"refusing to delete mounted smoke directory: {mounts}")
    shutil.rmtree(root)


def stop(processes, runtime=None):
    # The harness is a subreaper: helpers remain its descendants even when a
    # wrapper has already exited or a helper starts its own process group.
    with contextlib.ExitStack() as stack:
        owned = {}

        def capture_new(sig):
            with contextlib.ExitStack() as snapshot:
                for pid, fd in capture_processes(os.getpid(), snapshot):
                    if pid in owned and not select.select([owned[pid]], [], [], 0)[0]:
                        continue
                    if pid in owned:
                        os.close(owned.pop(pid))
                    owned[pid] = os.dup(fd)
                    with contextlib.suppress(ProcessLookupError):
                        signal.pidfd_send_signal(owned[pid], sig)

        def close_owned():
            for fd in owned.values():
                os.close(fd)

        stack.callback(close_owned)

        def signal_owned(sig):
            for fd in owned.values():
                with contextlib.suppress(ProcessLookupError):
                    signal.pidfd_send_signal(fd, sig)

        def reap():
            for process in processes:
                process.poll()
            direct = {p.pid for p in processes if p.returncode is None}
            for pid, fd in owned.items():
                if pid not in direct:
                    with contextlib.suppress(ChildProcessError):
                        os.waitid(os.P_PIDFD, fd, os.WEXITED | os.WNOHANG)

        def wait_stopped(sig):
            deadline = time.monotonic() + 3
            while True:
                capture_new(sig)
                reap()
                exited = len(select.select(list(owned.values()), [], [], 0)[0]) == len(
                    owned
                )
                if exited and (runtime is None or not mounts_under(runtime)):
                    reap()
                    if not child_pids(os.getpid()):
                        return True
                if time.monotonic() >= deadline:
                    return False
                time.sleep(0.05)

        # fusermount auto-unmount helpers ignore TERM and need time to observe
        # the portal's exit. Waiting only for the D-Bus parent kills them early.
        if wait_stopped(signal.SIGTERM):
            return
        errors = []
        if runtime is not None:
            try:
                detach_runtime_mounts(runtime)
            except (RuntimeError, OSError, subprocess.SubprocessError) as error:
                errors.append(str(error))
        signal_owned(signal.SIGKILL)
        if not wait_stopped(signal.SIGKILL):
            errors.append("owned processes or private mounts remain after cleanup")
        if errors:
            raise RuntimeError("; ".join(errors))


def run(args, root):
    prefix = args.prefix.resolve()
    cli = args.cli.resolve()
    token = secrets.token_hex(16)
    loaded = threading.Event()
    page_path = f"/page/{token}"
    callback_path = f"/loaded/{token}"

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path != page_path:
                self.send_error(404)
                return
            body = (
                "<!doctype html><title>Limux package smoke</title>"
                '<p id="proof">Packaged browser rendered this page.</p><script>'
                'window.addEventListener("load", () => {'
                f'fetch("{callback_path}", {{method: "POST", body: JSON.stringify({{'
                "url: location.href, ready: document.readyState,"
                'text: document.getElementById("proof").textContent'
                "})});});</script>"
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            if self.path != callback_path or not 0 < length < 4096:
                self.send_error(400)
                return
            try:
                payload = json.loads(self.rfile.read(length))
            except (ValueError, UnicodeDecodeError):
                self.send_error(400)
                return
            if payload != {
                "url": url,
                "ready": "complete",
                "text": "Packaged browser rendered this page.",
            }:
                self.send_error(400)
                return
            self.send_response(204)
            self.end_headers()
            loaded.set()

        def log_message(self, format, *values):
            print(f"browser HTTP: {format % values}", flush=True)

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    url = f"http://127.0.0.1:{server.server_port}{page_path}"
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    processes = []
    try:
        env = os.environ.copy()
        for key in list(env):
            if key.startswith(("LIMUX_", "GHOSTTY_", "WEBKIT_")) or key in {
                "DISPLAY",
                "WAYLAND_DISPLAY",
                "DBUS_SESSION_BUS_ADDRESS",
                "DBUS_STARTER_ADDRESS",
                "DBUS_STARTER_BUS_TYPE",
                "LD_LIBRARY_PATH",
                "LD_PRELOAD",
                "GDK_PIXBUF_MODULE_FILE",
                "GDK_PIXBUF_MODULEDIR",
                "GTK_PATH",
                "GTK_MODULES",
                "GTK4_MODULES",
                "http_proxy",
                "https_proxy",
                "all_proxy",
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "GNOME_KEYRING_CONTROL",
                "GPG_AGENT_INFO",
                "SSH_AUTH_SOCK",
            }:
                env.pop(key)
        for kind in ("DATA", "STATE", "CONFIG", "CACHE", "RUNTIME"):
            path = root / kind.lower()
            path.mkdir(mode=0o700)
            env[f"XDG_{kind}_HOME" if kind != "RUNTIME" else "XDG_RUNTIME_DIR"] = str(
                path
            )
        private_home = root / "home"
        private_home.mkdir(mode=0o700)
        dbus_config = root / "dbus.conf"
        dbus_config.write_text(
            """<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"""
        )
        env.update(
            {
                "HOME": str(private_home),
                "LIMUX_SOCKET": str(root / "control.sock"),
                "LIMUX_SOCKET_PATH": str(root / "control.sock"),
                "LIMUX_SOCKET_MODE": "runtime",
                "GDK_BACKEND": "wayland",
                "WAYLAND_DISPLAY": "wayland-package-smoke",
                "LIBGL_ALWAYS_SOFTWARE": "1",
                "GALLIUM_DRIVER": "llvmpipe",
                "LP_NUM_THREADS": "1",
                "XDG_DATA_DIRS": "/usr/local/share:/usr/share",
                "GTK_USE_PORTAL": "0",
                "GIO_USE_VFS": "local",
                "GTK_A11Y": "none",
            }
        )
        config = root / "config/ghostty"
        config.mkdir()
        (config / "config").write_text("command = /bin/sh\nfont-size = 12\n")
        session = {
            "version": 1,
            "workspaces": [
                {
                    "id": "00000000-0000-4000-8000-000000000001",
                    "name": "package-smoke",
                    "cwd": str(root),
                    "layout": {
                        "kind": "split",
                        "orientation": "horizontal",
                        "ratio": 0.5,
                        "start": {
                            "kind": "pane",
                            "pane_id": 1,
                            "active_tab_id": "terminal",
                            "tabs": [
                                {
                                    "id": "terminal",
                                    "tab_kind": "terminal",
                                    "cwd": str(root),
                                }
                            ],
                        },
                        "end": {
                            "kind": "pane",
                            "pane_id": 2,
                            "active_tab_id": "browser",
                            "tabs": [
                                {
                                    "id": "browser",
                                    "tab_kind": "browser",
                                    "uri": url,
                                    "custom_name": "Saved sandbox browser",
                                    "pinned": True,
                                }
                            ],
                        },
                    },
                }
            ],
        }
        (root / "data/limux").mkdir()
        (root / "data/limux/session.json").write_text(json.dumps(session))

        def start(command, name, child_env):
            with (root / f"{name}.log").open("w") as log:
                process = subprocess.Popen(
                    command,
                    env=child_env,
                    cwd=root,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
            processes.append(process)
            return process

        start(
            [
                "weston",
                "--backend=headless-backend.so",
                "--socket=" + env["WAYLAND_DISPLAY"],
                "--idle-time=0",
                "--width=1280",
                "--height=800",
            ],
            "weston",
            env,
        )
        wait_for(
            lambda: (root / "runtime" / env["WAYLAND_DISPLAY"]).is_socket(),
            processes,
            "Weston",
        )
        host_env = env.copy()
        if args.explicit_disable_sandbox:
            host_env["WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS"] = "1"
        print("Browser smoke verifies the packaged host's WebKit policy.", flush=True)
        if args.appimage:
            command = [str(prefix / "AppRun")]
        else:
            host_env["LD_LIBRARY_PATH"] = str(args.library_dir.resolve())
            host_env["GHOSTTY_RESOURCES_DIR"] = str(prefix / "share/limux/ghostty")
            host_env["TERMINFO"] = str(prefix / "share/limux/terminfo")
            command = [str(prefix / "libexec/limux/limux-host")]
        host_command = [
            "dbus-run-session",
            f"--config-file={dbus_config}",
            "--",
            *command,
        ]
        if args.deny_userns:
            host_command = [str(root / "deny-userns-launcher"), *host_command]
        host_process = start(host_command, "host", host_env)

        def control(*command):
            result = subprocess.run(
                [str(cli), *command],
                env=env,
                cwd=root,
                capture_output=True,
                text=True,
                timeout=5,
                check=False,
            )
            if result.returncode:
                raise RuntimeError(result.stderr.strip() or result.stdout.strip())
            return result.stdout

        surface = None

        def healthy():
            nonlocal surface
            try:
                result = json.loads(
                    control("--json", "surface-health", "--workspace", "package-smoke")
                )
            except (RuntimeError, subprocess.TimeoutExpired, ValueError):
                return False
            (root / "health.json").write_text(json.dumps(result, indent=2))
            for entry in result.get("surfaces", []):
                if (
                    entry.get("type") == "terminal"
                    and entry.get("healthy") is True
                    and entry.get("realized") is True
                    and entry.get("process_exited") is False
                    and all(
                        entry.get(key, 0) > 0
                        for key in ("columns", "rows", "width_px", "height_px")
                    )
                ):
                    surface = entry["surface_ref"]
                    return True
            return False

        wait_for(healthy, processes, "packaged terminal health")
        if args.expected_apparmor_profile:
            assert_apparmor_profile(host_process.pid, args.expected_apparmor_profile)
        target = ["--workspace", "package-smoke", "--surface", surface]
        proof = root / "terminal-proof"
        # The full marker is never typed, so readback cannot pass on command echo.
        control(
            "send",
            *target,
            f"printf '%s%s\\n' 'package-' '{token}'; printf ok > {shlex.quote(str(proof))}",
        )
        control("send-key", *target, "Enter")

        def terminal_output():
            output = control("read-screen", *target)
            (root / "screen.txt").write_text(output)
            return (
                proof.exists()
                and proof.read_text() == "ok"
                and f"package-{token}" in output
            )

        wait_for(
            terminal_output, processes, "packaged terminal command and screen readback"
        )
        if args.expect_browser_unavailable:
            def unavailable():
                try:
                    health = json.loads(
                        control("--json", "surface-health", "--workspace", "package-smoke")
                    )
                except (RuntimeError, subprocess.TimeoutExpired, ValueError):
                    return False
                (root / "health.json").write_text(json.dumps(health, indent=2))
                browser = next(
                    (
                        row
                        for row in health.get("surfaces", [])
                        if row.get("type") == "browser"
                    ),
                    None,
                )
                error = browser.get("error", "") if browser else ""
                return (
                    browser
                    if browser
                    and browser.get("healthy") is False
                    and browser.get("realized") is False
                    and browser.get("sandboxed") is False
                    and error
                    else False
                )

            browser_health = wait_for(
                unavailable,
                processes,
                "restored browser to report sandbox unavailable",
            )
            diagnostic = browser_health.get("error", "").lower()
            if not any(
                word in diagnostic for word in ("sandbox", "namespace", "bubblewrap", "bwrap")
            ):
                raise RuntimeError(
                    "browser sandbox failure was not reported clearly: "
                    f"{browser_health.get('error')}"
                )
            if loaded.is_set():
                raise RuntimeError("browser rendered while user namespaces were denied")
            assert_no_browser_sandbox_processes(host_process.pid)

            session_path = root / "data/limux/session.json"
            expected_browser_state = [
                ("browser", "Saved sandbox browser", True, url)
            ]
            if saved_browser_state(session_path) != expected_browser_state:
                raise RuntimeError(
                    "browser state was not preserved after sandbox denial: "
                    f"{saved_browser_state(session_path)}"
                )

            rejected_create = subprocess.run(
                [
                    str(cli),
                    "--json",
                    "new-surface",
                    "--workspace",
                    "package-smoke",
                    "--type",
                    "browser",
                    "--url",
                    url,
                ],
                env=env,
                cwd=root,
                capture_output=True,
                text=True,
                timeout=8,
                check=False,
            )
            create_error = f"{rejected_create.stderr}\n{rejected_create.stdout}".lower()
            if rejected_create.returncode == 0 or not any(
                word in create_error for word in ("sandbox", "namespace", "bubblewrap", "bwrap")
            ):
                raise RuntimeError(
                    "new browser creation did not fail with the sandbox diagnostic: "
                    f"{create_error.strip()}"
                )
            if saved_browser_state(session_path) != expected_browser_state:
                raise RuntimeError("rejected browser creation mutated saved session state")
            if not healthy():
                raise RuntimeError("terminal became unhealthy after browser sandbox denial")

            control(
                "send",
                *target,
                f"printf '%s%s\\n' 'denied-' '{token}'; printf ok > {shlex.quote(str(proof))}",
            )
            control("send-key", *target, "Enter")
            wait_for(
                lambda: proof.exists()
                and proof.read_text() == "ok"
                and f"denied-{token}" in control("read-screen", *target),
                processes,
                "terminal remains executable after browser sandbox denial",
            )

            old_host_process = host_process
            stop_process_tree(old_host_process)
            processes.remove(old_host_process)
            if saved_browser_state(session_path) != expected_browser_state:
                raise RuntimeError("browser URI, name, or pin changed during shutdown")
            host_process = start(host_command, "host-restarted", host_env)
            wait_for(healthy, processes, "terminal health after denied-browser restart")
            browser_health = wait_for(
                unavailable,
                processes,
                "restored browser to remain unavailable after restart",
            )
            if saved_browser_state(session_path) != expected_browser_state:
                raise RuntimeError("browser URI, name, or pin did not survive restart")
            assert_no_browser_sandbox_processes(host_process.pid)
            print(
                "Packaged denied-sandbox smoke: OK (browser unavailable, terminal usable, "
                "saved browser state retained across restart)",
                flush=True,
            )
        elif args.explicit_disable_sandbox:
            wait_for(
                loaded.is_set,
                processes,
                "browser page load and JavaScript callback with explicit bypass",
            )
            health = json.loads(
                control("--json", "surface-health", "--workspace", "package-smoke")
            )
            browser = next(
                (row for row in health.get("surfaces", []) if row.get("type") == "browser"),
                None,
            )
            if (
                not browser
                or browser.get("healthy") is not True
                or browser.get("realized") is not True
                or browser.get("sandboxed") is not False
            ):
                raise RuntimeError(
                    "explicit sandbox bypass was not reported as an unsandboxed browser: "
                    f"{browser}"
                )
            if not healthy():
                raise RuntimeError("terminal became unhealthy after explicit sandbox bypass")
            print(
                "Packaged explicit-bypass smoke: OK "
                "(browser works and health reports sandboxed=false)",
                flush=True,
            )
        else:
            wait_for(
                loaded.is_set,
                processes,
                "packaged browser page load and JavaScript callback",
            )
            verify_webkit_sandbox(host_process.pid)
            health = json.loads(
                control("--json", "surface-health", "--workspace", "package-smoke")
            )
            browser = next(
                (row for row in health.get("surfaces", []) if row.get("type") == "browser"),
                None,
            )
            if (
                not browser
                or browser.get("healthy") is not True
                or browser.get("realized") is not True
                or browser.get("sandboxed") is not True
            ):
                raise RuntimeError(
                    "sandboxed browser health did not report a healthy isolated surface: "
                    f"{browser}"
                )
            if not healthy():
                raise RuntimeError("terminal became unhealthy after browser load")
            print(
                "Packaged runtime smoke: OK (terminal health, executed input/readback, "
                "browser load/JavaScript and WebKit sandbox)",
                flush=True,
            )
    finally:
        failure = sys.exception()
        cleanup_errors = []
        try:
            stop(processes, root / "runtime")
        except (RuntimeError, OSError, subprocess.SubprocessError) as error:
            cleanup_errors.append(str(error))
        server.shutdown()
        server.server_close()
        thread.join(timeout=3)
        if cleanup_errors:
            message = "Runtime cleanup failed: " + "; ".join(cleanup_errors)
            if failure is None:
                raise RuntimeError(message)
            print(message, file=sys.stderr, flush=True)


def main():
    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "prefix", type=Path, help="extracted package prefix, or AppImage root"
    )
    parser.add_argument(
        "cli", type=Path, help="exact packaged CLI used for control commands"
    )
    parser.add_argument(
        "--library-dir", type=Path, help="packaged Ghostty directory for tar/deb/rpm"
    )
    parser.add_argument(
        "--appimage", action="store_true", help="launch the extracted AppRun unchanged"
    )
    parser.add_argument(
        "--deny-userns",
        action="store_true",
        help="deny user namespaces in the packaged host process tree with seccomp",
    )
    parser.add_argument(
        "--expect-browser-unavailable",
        action="store_true",
        help="expect the runtime policy to deny user namespaces and test graceful browser failure",
    )
    parser.add_argument(
        "--explicit-disable-sandbox",
        action="store_true",
        help="check an explicit WebKit sandbox bypass is reported as unsandboxed",
    )
    parser.add_argument(
        "--expected-apparmor-profile",
        help="require the limux-host process to have this AppArmor profile label",
    )
    args = parser.parse_args()
    if args.deny_userns and not args.explicit_disable_sandbox:
        args.expect_browser_unavailable = True
    if args.expect_browser_unavailable and args.explicit_disable_sandbox:
        parser.error(
            "--expect-browser-unavailable cannot be combined with --explicit-disable-sandbox"
        )
    if not args.appimage and args.library_dir is None:
        parser.error("--library-dir is required outside AppImage mode")
    try:
        require_pidfd_support()
    except RuntimeError as error:
        parser.error(str(error))
    for tool in (
        "weston",
        "dbus-run-session",
        "fusermount3",
        "bwrap",
        "xdg-dbus-proxy",
    ):
        if shutil.which(tool) is None:
            parser.error(f"missing runtime dependency: {tool}")
    if args.deny_userns and shutil.which("unshare") is None:
        parser.error("--deny-userns requires unshare from util-linux")
    root = Path(tempfile.mkdtemp(prefix="limux-package-runtime-"))
    try:
        if args.deny_userns:
            compile_userns_denial_launcher(root / "deny-userns-launcher")
        if args.expect_browser_unavailable or args.deny_userns:
            probe = [
                "bwrap",
                "--unshare-user",
                "--uid",
                "0",
                "--gid",
                "0",
                "--ro-bind",
                "/",
                "/",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--",
                "/bin/true",
            ]
            if args.deny_userns:
                probe.insert(0, str(root / "deny-userns-launcher"))
            result = subprocess.run(
                probe,
                capture_output=True,
                text=True,
                check=False,
                timeout=5,
            )
            (root / "userns-denial-probe.log").write_text(
                f"exit={result.returncode}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
            )
            if result.returncode == 0:
                parser.error(
                    "the negative test is not running under a user-namespace denial policy"
                )
            denial_text = f"{result.stdout}\n{result.stderr}".lower()
            if not any(
                reason in denial_text
                for reason in ("permission denied", "operation not permitted", "namespace")
            ):
                parser.error(
                    "bubblewrap failed for a reason other than user-namespace denial: "
                    f"{denial_text.strip()}"
                )
            print(
                "Runtime user-namespace denial confirmed: "
                f"{result.stderr.strip() or result.stdout.strip() or f'exit {result.returncode}'}",
                flush=True,
            )
        with child_subreaper():
            run(args, root)
        remove_runtime_directory(root)
    except BaseException as error:
        print(f"FAIL: {error}\nSmoke diagnostics retained at {root}", flush=True)
        for log in root.glob("*.log"):
            try:
                print(
                    f"== {log.name} ==\n{log.read_text(errors='replace')[-16000:]}",
                    flush=True,
                )
            except OSError as log_error:
                print(f"Cannot read {log}: {log_error}", flush=True)
        raise


if __name__ == "__main__":
    main()
