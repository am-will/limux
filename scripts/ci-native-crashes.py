#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Capture native failures from isolated GTK CI processes, without tracing them."""

import argparse
import hashlib
import json
import os
import resource
import subprocess
import sys
import tarfile
import time
from datetime import UTC, datetime
from pathlib import Path

CORE_LIMIT = 4 * 1024 * 1024 * 1024
OBJECT_LIMIT = 2 * 1024 * 1024 * 1024
MAX_CORES = 2
GDB = [
    "gdb",
    "-nx",
    "-nh",
    "-batch",
    "-iex",
    "set auto-load off",
    "-iex",
    "set debuginfod enabled off",
]


def setup(directory):
    if (
        os.environ.get("GITHUB_ACTIONS") != "true"
        or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"
    ):
        raise RuntimeError("kernel core setup is restricted to GitHub-hosted runners")
    cores = directory / "cores"
    cores.mkdir(parents=True, mode=0o700, exist_ok=True)
    pattern = str(cores / "core.%t.%p.%E")
    if len(pattern.encode()) >= 128:
        raise RuntimeError("core_pattern exceeds the kernel's filename template limit")
    previous = Path("/proc/sys/kernel/core_pattern").read_text().strip()
    subprocess.run(
        ["sudo", "sysctl", "-w", f"kernel.core_pattern={pattern}"], check=True
    )
    (directory / "capture.json").write_text(
        json.dumps(
            {
                "core_pattern": pattern,
                "previous_core_pattern": previous,
                "core_limit_bytes": CORE_LIMIT,
                "max_collected_cores": MAX_CORES,
                "object_limit_bytes": OBJECT_LIMIT,
            },
            indent=2,
        )
        + "\n"
    )


def run_command(directory, command):
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        raise RuntimeError("run requires a command after --")
    config = json.loads((directory / "capture.json").read_text())
    if (
        Path("/proc/sys/kernel/core_pattern").read_text().strip()
        != config["core_pattern"]
    ):
        raise RuntimeError("kernel core_pattern changed after diagnostics setup")
    if len(list((directory / "cores").glob("core.*"))) >= MAX_CORES:
        raise RuntimeError(
            "native core limit reached; inspect existing crashes before continuing"
        )
    resource.setrlimit(resource.RLIMIT_CORE, (CORE_LIMIT, CORE_LIMIT))
    with (directory / "commands.jsonl").open("a") as log:
        log.write(
            json.dumps(
                {
                    "started_at": datetime.now(UTC).isoformat(),
                    "pid": os.getpid(),
                    "cwd": os.getcwd(),
                    "argv": command,
                }
            )
            + "\n"
        )
    os.execvp(command[0], command)


def wait_for_cores(directory):
    previous = None
    stable = 0
    for _ in range(30):
        snapshot = [
            (path, path.stat().st_size, path.stat().st_mtime_ns)
            for path in sorted((directory / "cores").glob("core.*"))
        ]
        stable = stable + 1 if snapshot == previous else 0
        if stable >= 2:
            return [item[0] for item in snapshot]
        previous = snapshot
        time.sleep(1)
    raise RuntimeError("core files did not finish writing within 30 seconds")


def limit_gdb_output():
    resource.setrlimit(resource.RLIMIT_FSIZE, (16 * 1024 * 1024,) * 2)


def collect(directory):
    directory.mkdir(parents=True, exist_ok=True)
    errors = []
    records = []
    objects = set()
    virtual_objects = []
    cores = wait_for_cores(directory)
    if len(cores) > MAX_CORES:
        errors.append(
            f"found {len(cores)} cores; only the first {MAX_CORES} are analyzed"
        )
    for core in cores[:MAX_CORES]:
        record = {"core": core.name, "bytes": core.stat().st_size}
        records.append(record)
        try:
            executable = Path(core.name.split(".", 3)[3].replace("!", "/"))
            record["executable"] = str(executable)
            if not executable.is_absolute() or not executable.is_file():
                raise RuntimeError(f"matching executable is missing: {executable}")
            if record["bytes"] >= CORE_LIMIT:
                errors.append(
                    f"{core.name}: core reached the size limit and may be truncated"
                )
            trace = directory / f"{core.name}.backtrace.txt"
            commands = [
                "set pagination off",
                "set print elements 64",
                "set print max-depth 4",
                "set print repeats 8",
                "info program",
                "thread apply all bt full",
                "info registers",
                "info sharedlibrary",
                "info proc mappings",
                'python print("LIMUX_OBJECTS=" + __import__("json").dumps([o.filename for o in gdb.objfiles()]))',
            ]
            args = GDB + ["--se", str(executable), "--core", str(core)]
            for command in commands:
                args.extend(["-ex", command])
            with trace.open("w") as output:
                result = subprocess.run(
                    args,
                    check=False,
                    stdout=output,
                    stderr=subprocess.STDOUT,
                    timeout=60,
                    preexec_fn=limit_gdb_output,
                )
            text = trace.read_text(errors="replace")
            if result.returncode:
                raise RuntimeError(
                    f"GDB exited {result.returncode}; inspect {trace.name}"
                )
            if (
                "truncated" in text.lower()
                or "may not match specified executable" in text
            ):
                errors.append(
                    f"{core.name}: GDB reported incomplete or mismatched core data"
                )
            marker = next(
                line for line in text.splitlines() if line.startswith("LIMUX_OBJECTS=")
            )
            objects.update(json.loads(marker.removeprefix("LIMUX_OBJECTS=")))
            objects.add(str(executable))
            record["backtrace"] = trace.name
        except (
            OSError,
            RuntimeError,
            subprocess.TimeoutExpired,
            IndexError,
            StopIteration,
        ) as error:
            errors.append(f"{core.name}: {error}")

    manifest = []
    total_bytes = 0
    if objects:
        with tarfile.open(
            directory / "objects.tar.gz", "w:gz", dereference=True
        ) as archive:
            for filename in sorted(objects):
                path = Path(filename)
                if filename.startswith("system-supplied DSO at "):
                    virtual_objects.append(filename)
                    continue
                if not path.is_absolute() or not path.is_file():
                    errors.append(f"mapped object is unavailable: {filename}")
                    continue
                size = path.stat().st_size
                if total_bytes + size > OBJECT_LIMIT:
                    errors.append(f"object archive limit exceeded; omitted {filename}")
                    continue
                total_bytes += size
                with path.open("rb") as file:
                    digest = hashlib.file_digest(file, "sha256").hexdigest()
                archive.add(path, arcname=str(path).lstrip("/"), recursive=False)
                manifest.append({"path": str(path), "bytes": size, "sha256": digest})

    root = Path(__file__).resolve().parent.parent
    source_root = Path(os.environ.get("LIMUX_TEST_ROOT", root))
    provenance = source_root / "ghostty/zig-out/lib/limux-ghostty-build"
    ghostty_library = source_root / "ghostty/zig-out/lib/libghostty-internal.so"
    ghostty_debug_info = None
    if ghostty_library.is_file():
        try:
            sections = subprocess.check_output(
                ["readelf", "--wide", "--sections", str(ghostty_library)],
                text=True,
                timeout=10,
            )
            ghostty_debug_info = ".debug_info " in sections and ".symtab " in sections
        except (OSError, subprocess.SubprocessError) as error:
            errors.append(f"could not inspect Ghostty debug sections: {error}")
    metadata = {
        "cores": records,
        "objects": manifest,
        "virtual_objects": virtual_objects,
        "errors": errors,
        "source_sha": subprocess.check_output(
            ["git", "-C", str(source_root), "rev-parse", "HEAD"], text=True
        ).strip(),
        "ghostty_build": provenance.read_text() if provenance.is_file() else None,
        "ghostty_debug_info": ghostty_debug_info,
        "kernel": os.uname().release,
    }
    (directory / "manifest.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(
        f"Native crash diagnostics: {len(cores)} core(s), {len(errors)} collection error(s)"
    )
    for error in errors:
        print(f"::error::{error}", file=sys.stderr)
    if errors:
        return 1
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--directory",
        type=Path,
        default=Path(os.environ.get("RUNNER_TEMP", "/tmp")) / "limux-native-crashes",
    )
    parser.add_argument("action", choices=["setup", "run", "collect"])
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    directory = args.directory.resolve()
    try:
        if args.action == "setup":
            setup(directory)
        elif args.action == "run":
            run_command(directory, args.command)
        else:
            return collect(directory)
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"::error::Native crash diagnostics: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
