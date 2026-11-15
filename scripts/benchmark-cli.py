#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Compare two CLI builds using alternating pairs, without a shell.

Use an idle, disposable GTK host for --socket. The socket cases are read-only;
without it, only CLI startup is measured. Build both binaries with the same
profile and keep other builds/benchmarks stopped during measurement.
"""

import argparse
import datetime
import hashlib
import json
import math
import os
import platform
import resource
import statistics
import subprocess
import sys
import time
from pathlib import Path


def executable(value):
    path = Path(value).resolve()
    if not path.is_file() or not os.access(path, os.X_OK):
        raise argparse.ArgumentTypeError(f"not an executable file: {path}")
    return path


def positive(value):
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("must be at least 1")
    return number


def binary_metadata(path):
    with path.open("rb") as binary:
        digest = hashlib.file_digest(binary, "sha256").hexdigest()
    return {"path": str(path), "sha256": digest}


def measure(binary, arguments, env, timeout, expect_json):
    usage_before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started = time.perf_counter_ns()
    result = subprocess.run(
        [str(binary), *arguments],
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        check=True,
        timeout=timeout,
    )
    elapsed = time.perf_counter_ns() - started
    usage_after = resource.getrusage(resource.RUSAGE_CHILDREN)
    if expect_json:
        if not isinstance(json.loads(result.stdout), dict):
            raise RuntimeError("CLI response was not a JSON object")
    elif not result.stdout:
        raise RuntimeError("CLI help was empty")
    user = (usage_after.ru_utime - usage_before.ru_utime) * 1000
    system = (usage_after.ru_stime - usage_before.ru_stime) * 1000
    return {
        "wall_ms": elapsed / 1_000_000,
        "user_ms": user,
        "system_ms": system,
        "cpu_ms": user + system,
    }


def summarize(samples):
    summary = {}
    for metric in ("wall_ms", "cpu_ms", "user_ms", "system_ms"):
        values = sorted(sample[metric] for sample in samples)
        summary[metric] = {
            "median": statistics.median(values),
            "p95": values[math.ceil(len(values) * 0.95) - 1],
            "mean": statistics.mean(values),
        }
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=executable)
    parser.add_argument("after", type=executable)
    parser.add_argument("--socket", type=Path, help="private GTK host socket")
    parser.add_argument("--runs", type=positive, default=100, help="measured pairs")
    parser.add_argument("--warmup", type=positive, default=5, help="warmup pairs")
    parser.add_argument("--timeout", type=positive, default=10, help="seconds per call")
    parser.add_argument("--output", type=Path, help="JSON output file (default: stdout)")
    args = parser.parse_args()
    if args.socket is not None:
        args.socket = args.socket.resolve()
        if not args.socket.is_socket():
            parser.error(f"not a Unix socket: {args.socket}")

    binaries = {"before": args.before, "after": args.after}
    env = {key: value for key, value in os.environ.items() if not key.startswith("LIMUX_")}
    cases = {"help": (["--help"], False)}
    if args.socket is not None:
        scope = ["--socket", str(args.socket), "--json"]
        cases["identify"] = (scope + ["identify", "--no-caller"], True)
        cases["workspace.current"] = (
            scope
            + [
                "--request",
                '{"id":"benchmark","method":"workspace.current","params":{}}',
            ],
            True,
        )

    result = {
        "metadata": {
            "started_at": datetime.datetime.now(datetime.UTC).isoformat(),
            "platform": platform.platform(),
            "python": platform.python_version(),
            "cpu_count": os.cpu_count(),
            "cpu_affinity": sorted(os.sched_getaffinity(0)),
            "load_average_before": os.getloadavg(),
            "tokio_worker_threads": env.get("TOKIO_WORKER_THREADS"),
            "runs": args.runs,
            "warmup": args.warmup,
            "socket": str(args.socket) if args.socket else None,
            "cpu_scope": "CLI process and its children; excludes the GTK host",
            "binaries": {
                label: binary_metadata(path) for label, path in binaries.items()
            },
        },
        "cases": {},
    }
    for name, (arguments, expect_json) in cases.items():
        pairs = []
        for index in range(args.warmup + args.runs):
            order = ["before", "after"] if index % 2 == 0 else ["after", "before"]
            pair = {"order": order}
            for label in order:
                pair[label] = measure(
                    binaries[label], arguments, env, args.timeout, expect_json
                )
            if index >= args.warmup:
                pairs.append(pair)
        summaries = {
            label: summarize([pair[label] for pair in pairs]) for label in binaries
        }
        result["cases"][name] = {
            "arguments": arguments,
            "summary": summaries,
            "paired_wall_delta_ms_median": statistics.median(
                pair["after"]["wall_ms"] - pair["before"]["wall_ms"]
                for pair in pairs
            ),
            "pairs": pairs,
        }
        before = summaries["before"]["wall_ms"]["median"]
        after = summaries["after"]["wall_ms"]["median"]
        print(f"{name}: median {before:.3f} -> {after:.3f} ms", file=sys.stderr)
    result["metadata"]["load_average_after"] = os.getloadavg()
    output = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.write_text(output)
    else:
        sys.stdout.write(output)


if __name__ == "__main__":
    main()
