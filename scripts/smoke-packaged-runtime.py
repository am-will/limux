#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Exercise an extracted package without rebuilding or installing its binaries."""

import argparse
import contextlib
import http.server
import json
import os
import secrets
import shlex
import shutil
import signal
import subprocess
import tempfile
import threading
import time
from pathlib import Path


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


def descendants(pid):
    children = set()
    for path in Path(f"/proc/{pid}/task").glob("*/children"):
        with contextlib.suppress(FileNotFoundError, ProcessLookupError):
            children.update(int(value) for value in path.read_text().split())
    return children | {child for pid in children for child in descendants(pid)}


def stop(process):
    # PTY shells and WebKit helpers can start new process groups. Capture only
    # this test's descendants before terminating their parent D-Bus session.
    children = descendants(process.pid)
    for sig in (signal.SIGTERM, signal.SIGKILL):
        for pid in children:
            with contextlib.suppress(ProcessLookupError):
                os.kill(pid, sig)
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, sig)
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            continue
        if sig == signal.SIGTERM:
            time.sleep(0.1)


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
            "Browser smoke uses the packaged host's default sandbox policy.", flush=True
        )
        if args.appimage:
            command = [str(prefix / "AppRun")]
        else:
            host_env["LD_LIBRARY_PATH"] = str(args.library_dir.resolve())
            host_env["GHOSTTY_RESOURCES_DIR"] = str(prefix / "share/limux/ghostty")
            host_env["TERMINFO"] = str(prefix / "share/limux/terminfo")
            command = [str(prefix / "libexec/limux/limux-host")]
        start(["dbus-run-session", "--", *command], "host", host_env)

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
        if not healthy():
            raise RuntimeError("terminal became unhealthy after browser load")
        print(
            "Packaged runtime smoke: OK (terminal health, executed input/readback, browser load/JavaScript)"
        )
    finally:
        for process in reversed(processes):
            stop(process)
        server.shutdown()
        server.server_close()
        thread.join(timeout=3)


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
    for tool in ("weston", "dbus-run-session"):
        if shutil.which(tool) is None:
            parser.error(f"missing runtime dependency: {tool}")
    with tempfile.TemporaryDirectory(prefix="limux-package-runtime-") as directory:
        root = Path(directory)
        try:
            run(args, root)
        except (RuntimeError, OSError, subprocess.TimeoutExpired) as error:
            print(f"FAIL: {error}", flush=True)
            for log in root.glob("*.log"):
                print(
                    f"== {log.name} ==\n{log.read_text(errors='replace')[-16000:]}",
                    flush=True,
                )
            raise SystemExit(1) from error


if __name__ == "__main__":
    main()
