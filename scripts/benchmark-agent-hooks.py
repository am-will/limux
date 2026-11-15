#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Compare real hook persistence calls using isolated, synthetic session files."""

import argparse
import hashlib
import json
import math
import os
import platform
import re
import resource
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


def fixture(root, workspaces, tabs, recoveries, tombstones, expired):
    session_dir = root / "data" / "limux"
    hook_dir = root / "hooks"
    session_dir.mkdir(parents=True)
    hook_dir.mkdir()
    (root / "runtime").mkdir(mode=0o700)
    session = {"version": 1, "workspaces": []}
    for workspace in range(workspaces):
        tab_list = [
            {
                "id": f"workspace-{workspace}-tab-{tab}",
                "tab_kind": "terminal",
                "cwd": f"/tmp/benchmark/project-{workspace}/component-{tab}",
                "custom_name": f"Component {tab}",
                "pinned": False,
            }
            for tab in range(tabs)
        ]
        session["workspaces"].append(
            {
                "id": f"workspace-{workspace}",
                "name": f"Project {workspace}",
                "layout": {
                    "kind": "pane",
                    "pane_id": workspace + 1,
                    "active_tab_id": tab_list[0]["id"],
                    "tabs": tab_list,
                },
            }
        )
    raw = json.dumps(session, indent=2).encode()
    paths = [session_dir / "session.json"] + [
        session_dir / f"session-recovery-{index}.json" for index in range(recoveries)
    ]
    for path in paths:
        path.write_bytes(raw)
    timestamp = time.time()
    store = {
        "version": 1,
        "sessions": {},
        "ended_sessions": [
            {
                "session_id": f"ended-{index}",
                "workspace_id": "benchmark",
                "surface_id": f"1:ended-tab-{index}",
                "updated_at": 1.0 if expired else timestamp,
            }
            for index in range(tombstones)
        ],
    }
    env = {key: value for key, value in os.environ.items() if not key.startswith("LIMUX_")}
    env.update(
        HOME=str(root),
        XDG_DATA_HOME=str(root / "data"),
        XDG_STATE_HOME=str(root / "state"),
        XDG_CONFIG_HOME=str(root / "config"),
        XDG_CACHE_HOME=str(root / "cache"),
        XDG_RUNTIME_DIR=str(root / "runtime"),
        LIMUX_SOCKET=str(root / "unconnected.sock"),
        LIMUX_SESSION_DIR=str(session_dir),
        LIMUX_AGENT_HOOK_STATE_DIR=str(hook_dir),
        LIMUX_AGENT_LAUNCH_ARGV="codex",
    )
    return paths, hook_dir / "codex-hook-sessions.json", json.dumps(store), env


def run_hook(binary, env, trace=None, paths=()):
    command = [
        str(binary), "--json", "hooks", "codex", "PreToolUse",
        "--workspace", "benchmark", "--surface", "1:bench-tab",
    ]
    if trace:
        options = ["strace", "-f", "-qq", "-yy", "-e", "trace=openat,read"]
        for path in paths:
            options.extend(["-P", str(path)])
        command = options + ["-o", str(trace), "--"] + command
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    started = time.perf_counter_ns()
    result = subprocess.run(
        command, input='{"session_id":"bench-session"}', env=env,
        capture_output=True, text=True, timeout=30, check=True,
    )
    wall_ms = (time.perf_counter_ns() - started) / 1_000_000
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    if json.loads(result.stdout).get("continue") is not True:
        raise RuntimeError(f"unexpected hook result: {result.stdout}")
    persisted = json.loads((Path(env["LIMUX_AGENT_HOOK_STATE_DIR"]) / "codex-hook-sessions.json").read_text())
    record = persisted["sessions"]["bench-session"]
    if (record["workspace_id"], record["surface_id"]) != ("benchmark", "1:bench-tab"):
        raise RuntimeError("the hook did not persist the requested binding")
    return {
        "wall_ms": wall_ms,
        "cpu_ms": ((after.ru_utime + after.ru_stime) - (usage.ru_utime + usage.ru_stime)) * 1000,
    }


def summary(samples):
    result = {}
    for metric in ("wall_ms", "cpu_ms"):
        values = sorted(sample[metric] for sample in samples)
        result[metric] = {
            "median": statistics.median(values),
            "p95": values[math.ceil(len(values) * 0.95) - 1],
            "mean": statistics.mean(values),
        }
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--workspaces", type=int, default=20)
    parser.add_argument("--tabs", type=int, default=4)
    parser.add_argument("--recoveries", type=int, default=3)
    parser.add_argument("--tombstones", type=int, default=0)
    parser.add_argument("--expired", action="store_true")
    parser.add_argument("--runs", type=int, default=40)
    parser.add_argument("--warmup", type=int, default=3)
    parser.add_argument("--profile-only", action="store_true",
                        help="trace only reads of the synthetic session files; do not time calls")
    args = parser.parse_args()
    if min(args.workspaces, args.tabs, args.runs) < 1 or min(args.recoveries, args.tombstones, args.warmup) < 0:
        parser.error("workspaces, tabs, and runs must be positive; other counts must be nonnegative")
    if not args.after and not args.profile_only:
        parser.error("--after is required unless --profile-only is used")
    binaries = {"before": args.before.resolve()}
    if args.after:
        binaries["after"] = args.after.resolve()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    output = {
        "platform": platform.platform(), "cpu_count": os.cpu_count(),
        "load_average": os.getloadavg(),
        "fixture": {key: getattr(args, key) for key in ("workspaces", "tabs", "recoveries", "tombstones", "expired")},
        "binaries": {
            label: {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            for label, path in binaries.items()
        },
        "store_reset_before_each_call": True,
    }
    with tempfile.TemporaryDirectory(prefix="limux-hook-benchmark-") as directory:
        paths, store_path, store, env = fixture(
            Path(directory), args.workspaces, args.tabs, args.recoveries, args.tombstones, args.expired,
        )
        output["fixture"]["session_bytes"] = sum(path.stat().st_size for path in paths)
        if args.profile_only:
            output["profiles"] = {}
            for label, binary in binaries.items():
                store_path.write_text(store)
                trace = args.output.with_suffix(f".{label}.strace")
                run_hook(binary, env, trace, paths)
                text = trace.read_text()
                reads = re.findall(r"\bread\([^\n]*\)\s+= (\d+)$", text, re.MULTILINE)
                output["profiles"][label] = {
                    "trace": str(trace),
                    "session_opens": len(re.findall(r"\bopenat\(", text)),
                    "session_read_bytes": sum(map(int, reads)),
                }
        else:
            samples = {label: [] for label in binaries}
            for index in range(args.warmup + args.runs):
                labels = list(binaries)
                if index % 2:
                    labels.reverse()
                for label in labels:
                    store_path.write_text(store)
                    sample = run_hook(binaries[label], env)
                    if index >= args.warmup:
                        samples[label].append(sample)
            output["samples"] = samples
            output["summary"] = {label: summary(values) for label, values in samples.items()}
    args.output.write_text(json.dumps(output, indent=2) + "\n")
    print(json.dumps(output.get("profiles", output.get("summary")), indent=2))


if __name__ == "__main__":
    main()
