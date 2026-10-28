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
        if check():
            return
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
        if "WebKitWebProcess" in executable:
            web_processes.append(pid)
    if not web_processes:
        raise RuntimeError(
            "could not locate the WebKitWebProcess below the packaged host; "
            "its sandbox state could not be verified"
        )

    host_user_namespace = os.stat("/proc/self/ns/user").st_ino
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
            "separate_user_namespace": user_namespace != host_user_namespace,
        }
        if (
            sandbox["NoNewPrivs"] != "1"
            or sandbox["Seccomp"] != "2"
            or not sandbox["separate_user_namespace"]
        ):
            raise RuntimeError(
                f"WebKitWebProcess {pid} did not enter the default sandbox: {sandbox}"
            )
        print(f"WebKitWebProcess {pid} sandbox: {sandbox}", flush=True)
    if not inspected:
        raise RuntimeError("WebKitWebProcess exited before sandbox inspection")


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
            }:
                env.pop(key)
        for kind in ("DATA", "STATE", "CONFIG", "CACHE", "RUNTIME"):
            path = root / kind.lower()
            path.mkdir(mode=0o700)
            env[f"XDG_{kind}_HOME" if kind != "RUNTIME" else "XDG_RUNTIME_DIR"] = str(
                path
            )
        env.update(
            {
                "LIMUX_SOCKET": str(root / "control.sock"),
                "LIMUX_SOCKET_PATH": str(root / "control.sock"),
                "LIMUX_SOCKET_MODE": "runtime",
                "GDK_BACKEND": "wayland",
                "WAYLAND_DISPLAY": "wayland-package-smoke",
                "LIBGL_ALWAYS_SOFTWARE": "1",
                "GALLIUM_DRIVER": "llvmpipe",
                "LP_NUM_THREADS": "1",
                "XDG_DATA_DIRS": "/usr/local/share:/usr/share",
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
                                {"id": "browser", "tab_kind": "browser", "uri": url}
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
        print(
            "Browser smoke verifies the packaged host's default WebKit sandbox.",
            flush=True,
        )
        if args.appimage:
            command = [str(prefix / "AppRun")]
        else:
            host_env["LD_LIBRARY_PATH"] = str(args.library_dir.resolve())
            host_env["GHOSTTY_RESOURCES_DIR"] = str(prefix / "share/limux/ghostty")
            host_env["TERMINFO"] = str(prefix / "share/limux/terminfo")
            command = [str(prefix / "libexec/limux/limux-host")]
        host_process = start(["dbus-run-session", "--", *command], "host", host_env)

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
        wait_for(
            loaded.is_set,
            processes,
            "packaged browser page load and JavaScript callback",
        )
        verify_webkit_sandbox(host_process.pid)
        if not healthy():
            raise RuntimeError("terminal became unhealthy after browser load")
        print(
            "Packaged runtime smoke: OK (terminal health, executed input/readback, browser load/JavaScript)"
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
    args = parser.parse_args()
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
    root = Path(tempfile.mkdtemp(prefix="limux-package-runtime-"))
    try:
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
