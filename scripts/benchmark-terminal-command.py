#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Compare command-ready latency in isolated GTK hosts built from two revisions.

Requires Weston, D-Bus, and a built Ghostty prefix. Use matching build profiles
and stop other benchmark/build jobs during measurement. Hosts use private XDG
directories and software rendering. Execution latency is observed after the
reply, so it includes reply latency when the command has already executed.
"""

import argparse
import contextlib
import datetime
import hashlib
import importlib.util
import json
import math
import os
import platform
import shlex
import socket
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


def load_smoke_helpers():
    path = Path(__file__).with_name("smoke-packaged-runtime.py")
    spec = importlib.util.spec_from_file_location("limux_package_smoke", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def rpc(path, method, params=None):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(10)
        connection.connect(str(path))
        request = {"id": "benchmark", "method": method, "params": params or {}}
        connection.sendall(json.dumps(request).encode() + b"\n")
        with connection.makefile("rb") as response:
            payload = json.loads(response.readline())
    if not payload.get("ok"):
        raise RuntimeError(f"{method}: {payload}")
    return payload["result"]


def wait_until(check, processes, description, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for process in processes:
            if process.poll() is not None:
                raise RuntimeError(f"{description}: child exited {process.returncode}")
        if value := check():
            return value
        time.sleep(0.002)
    raise RuntimeError(f"timed out waiting for {description}")


@contextlib.contextmanager
def private_host(binary, ghostty_prefix):
    """Use the maintained smoke harness's pidfd/subreaper cleanup."""
    smoke = load_smoke_helpers()
    smoke.require_pidfd_support()
    root = Path(tempfile.mkdtemp(prefix="limux-command-bench-"))
    processes = []
    env = os.environ.copy()
    for key in list(env):
        if key.startswith(("LIMUX_", "GHOSTTY_", "WEBKIT_")) or key in {
            "DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS",
            "DBUS_STARTER_ADDRESS", "DBUS_STARTER_BUS_TYPE", "LD_PRELOAD",
            "LD_LIBRARY_PATH", "GTK_PATH", "GTK_MODULES", "GTK4_MODULES",
            "GDK_PIXBUF_MODULE_FILE", "GDK_PIXBUF_MODULEDIR", "SSH_AUTH_SOCK",
        }:
            env.pop(key)
    for kind in ("DATA", "STATE", "CONFIG", "CACHE", "RUNTIME"):
        path = root / kind.lower()
        path.mkdir(mode=0o700)
        env[f"XDG_{kind}_HOME" if kind != "RUNTIME" else "XDG_RUNTIME_DIR"] = str(path)
    (root / "home").mkdir()
    config = root / "config/ghostty"
    config.mkdir()
    (config / "config").write_text(
        "command = direct:/bin/sh\nshell-integration = none\nfont-size = 12\n"
    )
    (root / "dbus.conf").write_text(
        '<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>'
        '<auth>EXTERNAL</auth><policy context="default">'
        '<allow send_destination="*" eavesdrop="true"/>'
        '<allow eavesdrop="true"/><allow own="*"/></policy></busconfig>'
    )
    env.update({
        "HOME": str(root / "home"), "LIMUX_SOCKET": str(root / "control.sock"),
        "LIMUX_SOCKET_PATH": str(root / "control.sock"), "LIMUX_SOCKET_MODE": "runtime",
        "GDK_BACKEND": "wayland", "WAYLAND_DISPLAY": "wayland-benchmark",
        "LIBGL_ALWAYS_SOFTWARE": "1", "GALLIUM_DRIVER": "llvmpipe", "LP_NUM_THREADS": "1",
        "GTK_USE_PORTAL": "0", "GIO_USE_VFS": "local", "GTK_A11Y": "none",
        "XDG_DATA_DIRS": "/usr/local/share:/usr/share", "SHELL": "/bin/sh",
        "ENV": "/dev/null", "PS1": "", "PS2": "",
        "LD_LIBRARY_PATH": str(ghostty_prefix / "lib"),
        "GHOSTTY_RESOURCES_DIR": str(ghostty_prefix / "share/ghostty"),
        "TERMINFO": str(ghostty_prefix / "share/terminfo"),
    })

    def start(command, name):
        with (root / f"{name}.log").open("w") as log:
            process = subprocess.Popen(command, env=env, cwd=root, stdout=log,
                                       stderr=subprocess.STDOUT, start_new_session=True)
        processes.append(process)
        return process

    with smoke.child_subreaper():
        try:
            start(["weston", "--backend=headless-backend.so", "--socket=wayland-benchmark",
                   "--idle-time=0", "--width=960", "--height=640"], "weston")
            wait_until(lambda: (root / "runtime/wayland-benchmark").is_socket(),
                       processes, "Weston")
            launched = time.perf_counter_ns()
            start(["dbus-run-session", f"--config-file={root / 'dbus.conf'}", "--", str(binary)], "host")
            path = root / "control.sock"
            wait_until(path.is_socket, processes, "control socket")
            rpc(path, "workspace.current")
            startup_ms = (time.perf_counter_ns() - launched) / 1e6
            yield root, path, processes, startup_ms
        except Exception:
            log = root / "host.log"
            if log.exists():
                print(log.read_text()[-6000:], flush=True)
            raise
        finally:
            smoke.stop(processes, root / "runtime")
            smoke.remove_runtime_directory(root)


def measure(binary, ghostty_prefix, iterations):
    samples = []
    with private_host(binary, ghostty_prefix) as (root, path, processes, startup_ms):
        rpc(path, "workspace.current")
        for index in range(iterations + 2):
            marker = root / f"executed-{index}"
            command = f"printf 'executed\\n' >> {shlex.quote(str(marker))}"
            started = time.perf_counter_ns()
            rpc(path, "workspace.create", {
                "name": f"benchmark-{index}", "cwd": str(root), "command": command,
            })
            reply_ms = (time.perf_counter_ns() - started) / 1e6
            wait_until(marker.exists, processes, "terminal command")
            executed_ms = (time.perf_counter_ns() - started) / 1e6
            # Let queued terminal input drain before checking exactly-once execution.
            time.sleep(0.03)
            if marker.read_text() != "executed\n":
                raise RuntimeError("terminal command did not execute exactly once")
            if index >= 2:
                samples.append({"reply_ms": reply_ms, "executed_ms": executed_ms})
    return {"startup_ms": startup_ms, "samples": samples}


def summarize(values):
    ordered = sorted(values)
    return {"median_ms": statistics.median(values),
            "p95_ms": ordered[math.ceil(len(ordered) * 0.95) - 1],
            "min_ms": ordered[0], "max_ms": ordered[-1]}


def binary_metadata(path):
    with path.open("rb") as binary:
        digest = hashlib.file_digest(binary, "sha256").hexdigest()
    return {"path": str(path), "sha256": digest}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", required=True, type=Path)
    parser.add_argument("--after", required=True, type=Path)
    parser.add_argument("--ghostty-prefix", required=True, type=Path,
                        help="Built Ghostty prefix containing lib/ and share/")
    parser.add_argument("--rounds", type=int, default=4)
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if args.rounds < 2 or args.iterations < 1:
        parser.error("use at least two rounds and one iteration")
    binaries = {name: getattr(args, name).resolve() for name in ("before", "after")}
    output = {
        "started_at": datetime.datetime.now(datetime.UTC).isoformat(),
        "kernel": platform.release(), "cpu_affinity": sorted(os.sched_getaffinity(0)),
        "loadavg_start": os.getloadavg(), "renderer": "llvmpipe",
        "binaries": {name: binary_metadata(path) for name, path in binaries.items()},
        "rounds": [],
    }
    for round_index in range(args.rounds):
        order = ("before", "after") if round_index % 2 == 0 else ("after", "before")
        for variant in order:
            result = measure(binaries[variant], args.ghostty_prefix.resolve(), args.iterations)
            output["rounds"].append({"round": round_index, "variant": variant, **result})
            print(json.dumps({"round": round_index, "variant": variant,
                              **summarize([s["reply_ms"] for s in result["samples"]])}), flush=True)
    output["loadavg_end"] = os.getloadavg()
    output["summary"] = {}
    for variant in binaries:
        samples = [s for r in output["rounds"] if r["variant"] == variant for s in r["samples"]]
        output["summary"][variant] = {metric: summarize([s[metric] for s in samples])
                                      for metric in ("reply_ms", "executed_ms")}
    args.output.write_text(json.dumps(output, indent=2) + "\n")
    print(json.dumps(output["summary"], indent=2))


if __name__ == "__main__":
    main()
