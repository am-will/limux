#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["python-xlib==0.33"]
# ///
"""Compile the real GTK seat guard and exercise removal on a private Xvfb."""
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import tempfile
import time
import tomllib

from Xlib import X, XK, display
from Xlib.ext import xtest
from Xlib.protocol import rq

ROOT = Path(__file__).resolve().parents[2]


class SetClientPointer(rq.Request):
    _request = rq.Struct(
        rq.Card8("opcode"), rq.Opcode(44), rq.RequestLength(),
        rq.Card32("window"), rq.Card16("deviceid"), rq.Pad(2),
    )


def build_consumer(directory):
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    version = next(p["version"] for p in lock["package"] if p["name"] == "gtk4")
    manifest = Path(directory) / "Cargo.toml"
    manifest.write_text(
        '[package]\nname = "limux-input-seat-regression"\nversion = "0.0.0"\nedition = "2021"\n'
        f'[dependencies]\ngtk4 = {{ version = "={version}", features = ["v4_10"] }}\n'
        '[[bin]]\nname = "input-seat-window"\n'
        f'path = {json.dumps(str(ROOT / "scripts/tests/input-seat-window.rs"))}\n'
    )
    shutil.copyfile(ROOT / "Cargo.lock", manifest.with_name("Cargo.lock"))
    for command in ("test", "build"):
        subprocess.run(
            ["cargo", command, "--offline", "--manifest-path", str(manifest),
             "--target-dir", str(ROOT / "target"), "--bin", "input-seat-window"],
            env={**os.environ, "PKG_CONFIG": "/usr/bin/pkg-config"}, check=True,
        )
    return ROOT / "target/debug/input-seat-window"


def exercise(binary, display_name):
    env = {**os.environ, "DISPLAY": display_name, "GDK_BACKEND": "x11",
           "GSK_RENDERER": "cairo", "GTK_A11Y": "none", "LIMUX_INPUT_SEAT_TEST": "1"}

    def xinput(*args):
        return subprocess.check_output(["xinput", *args], env=env, text=True).strip()

    # Cover a seat already present at startup as well as later additions.
    # Keep one connection open so Xvfb cannot reset between xinput and GTK.
    core = display.Display(display_name)
    xinput("--create-master", f"Limux regression {os.getpid()} 0")
    gui = subprocess.Popen([binary], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        assert select.select([gui.stdout], [], [], 8)[0], "GTK window did not become ready"
        line = gui.stdout.readline()
        assert line, f"GTK startup failed: {gui.stderr.read()}"
        window = json.loads(line)["ready"]

        for cycle in range(5):
            name = f"Limux regression {os.getpid()} {cycle}"
            if cycle:
                xinput("--create-master", name)
            pointer = int(xinput("--list", "--id-only", name + " pointer"))
            private = display.Display(display_name)
            try:
                opcode = private.query_extension("XInputExtension").major_opcode
                SetClientPointer(display=private.display, opcode=opcode, window=X.NONE, deviceid=pointer)
                xtest.fake_input(private, X.MotionNotify, x=900, y=600)
                private.sync()
                time.sleep(0.15)
                xtest.fake_input(private, X.MotionNotify, x=100, y=100)
                private.sync()
                time.sleep(0.3)
                xinput("--remove-master", str(pointer), "Floating")
            finally:
                private.close()
            time.sleep(0.1)
            core.set_input_focus(window, X.RevertToParent, X.CurrentTime)
            xtest.fake_input(core, X.MotionNotify, x=900, y=600)
            core.sync()
            time.sleep(0.03)
            xtest.fake_input(core, X.MotionNotify, x=120, y=120)
            xtest.fake_input(core, X.ButtonPress, detail=1)
            xtest.fake_input(core, X.ButtonRelease, detail=1)
            keycode = core.keysym_to_keycode(XK.string_to_keysym("a"))
            xtest.fake_input(core, X.KeyPress, detail=keycode)
            xtest.fake_input(core, X.KeyRelease, detail=keycode)
            core.sync()
            time.sleep(0.1)
        stdout, stderr = gui.communicate(timeout=15)
        assert gui.returncode == 0, f"GTK exited {gui.returncode}: {stderr}\nEvents: {stdout}"
        events = [json.loads(line) for line in stdout.splitlines()]
        assert sum(e["event"] == "click" for e in events) == 5, events
        assert sum(e["event"] == "key" and e["value"] == 97 for e in events) == 5, events
        assert any(e["event"] == "cursor-updates" and e["count"] >= 400 for e in events), events
        assert not any(error in stderr for error in ("CRITICAL", "Gdk-WARNING", "XI_BadDevice")), stderr
        print("Passed: 5 seat removals, 5 core clicks, 5 core keys, 500 cursor updates, clean GTK shutdown.")
    finally:
        core.close()
        if gui.poll() is None:
            gui.terminate()
        gui.communicate(timeout=5)


def main():
    with tempfile.TemporaryDirectory(prefix="limux-input-seat-") as directory:
        binary = build_consumer(directory)
        read_fd, write_fd = os.pipe()
        xvfb = subprocess.Popen(
            ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1280x720x24", "-nolisten", "tcp"],
            pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        )
        os.close(write_fd)
        try:
            with os.fdopen(read_fd) as pipe:
                assert select.select([pipe], [], [], 8)[0], "Private Xvfb did not start"
                number = pipe.readline().strip()
            if not number.isdigit() or xvfb.poll() is not None:
                raise RuntimeError(f"Refusing display {number!r}")
            exercise(binary, ":" + number)
        finally:
            xvfb.terminate()
            xvfb.communicate(timeout=5)


if __name__ == "__main__":
    main()
