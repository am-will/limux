#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Regression tests for packaged-smoke process and mount cleanup."""

import contextlib
import ctypes
import importlib.util
import io
import os
import select
import signal
import subprocess
import sys
import tempfile
import traceback
import unittest
from pathlib import Path
from unittest import mock

SPEC = importlib.util.spec_from_file_location(
    "smoke", Path(__file__).resolve().parents[1] / "smoke-packaged-runtime.py"
)
smoke = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(smoke)


class CleanupTests(unittest.TestCase):
    def test_unsupported_python_fails_before_creating_runtime_or_children(self):
        for module, name in (
            (os, "pidfd_open"),
            (os, "P_PIDFD"),
            (os, "waitid"),
            (signal, "pidfd_send_signal"),
        ):
            with (
                self.subTest(api=name),
                mock.patch.object(module, name, None),
                mock.patch.object(
                    sys, "argv", ["smoke", ".", ".", "--library-dir", "."]
                ),
                mock.patch.object(smoke.subprocess, "Popen") as launch,
                mock.patch.object(smoke.tempfile, "mkdtemp") as create_runtime,
                contextlib.redirect_stderr(io.StringIO()) as diagnostics,
            ):
                with self.assertRaises(SystemExit) as error:
                    smoke.main()
                self.assertEqual(error.exception.code, 2)
                self.assertIn(name, diagnostics.getvalue())
                launch.assert_not_called()
                create_runtime.assert_not_called()

    def test_subreaper_setting_is_restored(self):
        libc = ctypes.CDLL(None)
        before = ctypes.c_int()
        after = ctypes.c_int()
        self.assertEqual(libc.prctl(37, ctypes.byref(before), 0, 0, 0), 0)
        with smoke.child_subreaper():
            self.assertEqual(libc.prctl(37, ctypes.byref(after), 0, 0, 0), 0)
            self.assertEqual(after.value, 1)
        self.assertEqual(libc.prctl(37, ctypes.byref(after), 0, 0, 0), 0)
        self.assertEqual(after.value, before.value)

    def test_cleans_up_child_after_wrapper_was_reaped(self):
        for escaped_session in (False, True):
            with self.subTest(escaped_session=escaped_session):
                pid = os.fork()
                if pid == 0:
                    try:
                        self.exited_wrapper_case(escaped_session)
                    except BaseException:  # noqa: BLE001 - the fork must not run the parent suite
                        traceback.print_exc()
                        os._exit(1)
                    os._exit(0)
                _, status = os.waitpid(pid, 0)
                self.assertEqual(os.waitstatus_to_exitcode(status), 0)

    def exited_wrapper_case(self, escaped_session):
        # Isolate the subreaper setting in the forked test process. The original
        # implementation still misses this adopted child during run() cleanup.
        self.assertEqual(ctypes.CDLL(None).prctl(36, 1, 0, 0, 0), 0)
        child_code = "import os, signal; print(os.getpid(), flush=True); signal.pause()"
        wrapper_code = (
            "import subprocess, sys; "
            "subprocess.Popen([sys.executable, '-c', sys.argv[1]], "
            f"start_new_session={escaped_session!r})"
        )
        wrapper = subprocess.Popen(
            [sys.executable, "-c", wrapper_code, child_code],
            stdout=subprocess.PIPE,
            start_new_session=True,
        )
        child_fd = None
        try:
            self.assertTrue(select.select([wrapper.stdout], [], [], 5)[0])
            child_fd = os.pidfd_open(int(wrapper.stdout.readline()))
            wrapper.wait(timeout=5)
            self.assertFalse(select.select([child_fd], [], [], 0)[0])
            with (
                tempfile.TemporaryDirectory() as directory,
                mock.patch.object(smoke.http.server, "ThreadingHTTPServer"),
                mock.patch.object(smoke.subprocess, "Popen", return_value=wrapper),
                mock.patch.object(
                    smoke, "wait_for", side_effect=RuntimeError("wrapper exited")
                ),
            ):
                root = Path(directory)
                args = mock.Mock(
                    prefix=root,
                    cli=root,
                    library_dir=root,
                    appimage=False,
                    deny_userns=False,
                    expect_browser_unavailable=False,
                    explicit_disable_sandbox=False,
                    expected_apparmor_profile=None,
                )
                with self.assertRaisesRegex(RuntimeError, "wrapper exited"):
                    smoke.run(args, root)
            self.assertTrue(
                select.select([child_fd], [], [], 2)[0],
                "helper survived cleanup after its wrapper exited",
            )
            self.assertEqual(smoke.child_pids(os.getpid()), set())
        finally:
            if child_fd is not None:
                with contextlib.suppress(ProcessLookupError):
                    signal.pidfd_send_signal(child_fd, signal.SIGKILL)
                with contextlib.suppress(ChildProcessError):
                    os.waitid(os.P_PIDFD, child_fd, os.WEXITED)
                os.close(child_fd)
            if wrapper.poll() is None:
                wrapper.kill()
            wrapper.wait(timeout=5)
            wrapper.stdout.close()

    def test_functional_error_survives_cleanup_failure(self):
        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(smoke.http.server, "ThreadingHTTPServer"),
            mock.patch.object(smoke.subprocess, "Popen"),
            mock.patch.object(
                smoke,
                "wait_for",
                side_effect=[None, RuntimeError("functional failure")],
            ),
            mock.patch.object(
                smoke, "stop", side_effect=RuntimeError("cleanup failure")
            ) as stop,
            contextlib.redirect_stderr(io.StringIO()) as diagnostics,
        ):
            root = Path(directory)
            args = mock.Mock(
                prefix=root,
                cli=root,
                library_dir=root,
                appimage=False,
                deny_userns=False,
                expect_browser_unavailable=False,
                explicit_disable_sandbox=False,
                expected_apparmor_profile=None,
            )
            with self.assertRaisesRegex(RuntimeError, "functional failure"):
                smoke.run(args, root)
            self.assertEqual(len(stop.call_args.args[0]), 2)
            self.assertIn("cleanup failure", diagnostics.getvalue())

    def test_rejects_a_recycled_descendant_pid(self):
        with (
            contextlib.ExitStack() as stack,
            mock.patch.object(smoke, "child_pids", return_value={20}),
            mock.patch.object(os, "pidfd_open", side_effect=[100, 200]),
            mock.patch.object(os, "close") as close,
            mock.patch.object(Path, "read_text", return_value="20 (unrelated) S 99 0"),
        ):
            self.assertEqual(smoke.capture_processes(10, stack), [])
            stack.close()
            close.assert_has_calls([mock.call(200), mock.call(100)])

    def test_waits_for_descendant_cleanup_after_parent_exits(self):
        child_code = """
import os, signal, sys, time
from pathlib import Path
signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGTERM})
print(os.getpid(), flush=True)
sys.stdin.read()
time.sleep(0.3)
Path(sys.argv[1]).write_text('unmounted')
"""
        parent_code = """
import signal, subprocess, sys
child = subprocess.Popen([sys.executable, '-c', sys.argv[1], sys.argv[2]],
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         start_new_session=True)
print(child.stdout.readline().decode().strip(), flush=True)
signal.pause()
"""
        with smoke.child_subreaper(), tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / "cleanup-finished"
            process = subprocess.Popen(
                [sys.executable, "-c", parent_code, child_code, str(marker)],
                stdout=subprocess.PIPE,
                start_new_session=True,
            )
            child_fd = None
            try:
                self.assertTrue(select.select([process.stdout], [], [], 5)[0])
                child_pid = int(process.stdout.readline())
                child_fd = os.pidfd_open(child_pid)
                smoke.stop([process])
                self.assertEqual(marker.read_text(), "unmounted")
            finally:
                if child_fd is not None:
                    with contextlib.suppress(ProcessLookupError):
                        signal.pidfd_send_signal(child_fd, signal.SIGKILL)
                    os.close(child_fd)
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=5)
                process.stdout.close()


class MountTests(unittest.TestCase):
    def setUp(self):
        self.runtime = Path("/tmp/limux smoke/runtime")

    def mount(self, name="doc", kind="fuse.portal", source="portal", uid=None):
        return (
            self.runtime / name,
            kind,
            source,
            [f"user_id={os.getuid() if uid is None else uid}"],
        )

    def test_reads_mountinfo_without_traversing_disconnected_mounts(self):
        mountinfo = (
            "100 1 0:1 / /run/user/1000/doc rw - fuse.portal portal rw,user_id=1000\n"
            "101 1 0:2 / /tmp/limux\\040smoke/runtime/doc rw - fuse.portal portal rw,user_id=1000\n"
            "102 1 0:3 / /tmp/limux\\040smoke/runtime-other/doc rw - fuse.portal portal rw,user_id=1000\n"
        )
        with (
            mock.patch.object(Path, "read_text", return_value=mountinfo),
            mock.patch.object(Path, "stat", side_effect=OSError(107, "disconnected")),
        ):
            self.assertEqual(
                smoke.mounts_under(self.runtime),
                [
                    (
                        self.runtime / "doc",
                        "fuse.portal",
                        "portal",
                        ["rw", "user_id=1000"],
                    )
                ],
            )

    def test_waits_for_natural_unmount_after_process_exit(self):
        process = mock.Mock()
        process.poll.return_value = 0
        with (
            mock.patch.object(
                smoke, "mounts_under", side_effect=[[self.mount()], [self.mount()], []]
            ),
            mock.patch.object(smoke.time, "sleep"),
            mock.patch.object(smoke, "detach_runtime_mounts") as detach,
        ):
            smoke.stop([process], self.runtime)
            detach.assert_not_called()

    def test_detaches_only_verified_private_fuse_mounts(self):
        mounts = [self.mount(), self.mount("gvfs", "fuse.gvfsd-fuse", "gvfsd-fuse")]
        with (
            mock.patch.object(smoke, "mounts_under", return_value=mounts),
            mock.patch.object(smoke.subprocess, "run") as unmount,
        ):
            smoke.detach_runtime_mounts(self.runtime)
        self.assertEqual(
            unmount.call_args_list,
            [
                mock.call(
                    ["fusermount3", "-u", "-z", "--", str(self.runtime / name)],
                    check=True,
                    timeout=5,
                )
                for name in ("doc", "gvfs")
            ],
        )

    def test_rejects_unknown_mounts_before_unmounting_anything(self):
        for mount in (
            self.mount("unexpected"),
            self.mount("doc/child"),
            self.mount(kind="fuse.other"),
            self.mount(source="other"),
            self.mount(uid=os.getuid() + 1),
        ):
            with (
                self.subTest(mount=mount),
                mock.patch.object(
                    smoke, "mounts_under", return_value=[self.mount(), mount]
                ),
                mock.patch.object(smoke.subprocess, "run") as unmount,
            ):
                with self.assertRaisesRegex(RuntimeError, "unexpected private mount"):
                    smoke.detach_runtime_mounts(self.runtime)
                unmount.assert_not_called()

    def test_unmount_failure_is_not_ignored(self):
        with (
            mock.patch.object(smoke, "mounts_under", return_value=[self.mount()]),
            mock.patch.object(
                smoke.subprocess,
                "run",
                side_effect=subprocess.CalledProcessError(1, "fusermount3"),
            ),
            self.assertRaises(subprocess.CalledProcessError),
        ):
            smoke.detach_runtime_mounts(self.runtime)

    def test_auto_unmount_winning_the_race_is_success(self):
        with (
            mock.patch.object(smoke, "mounts_under", side_effect=[[self.mount()], []]),
            mock.patch.object(
                smoke.subprocess,
                "run",
                side_effect=subprocess.CalledProcessError(1, "fusermount3"),
            ),
        ):
            smoke.detach_runtime_mounts(self.runtime)

    def test_refuses_to_delete_a_still_mounted_directory(self):
        with (
            mock.patch.object(smoke, "mounts_under", return_value=[self.mount()]),
            mock.patch.object(smoke.shutil, "rmtree") as remove,
        ):
            with self.assertRaisesRegex(RuntimeError, "refusing to delete mounted"):
                smoke.remove_runtime_directory(self.runtime.parent)
            remove.assert_not_called()


if __name__ == "__main__":
    try:
        smoke.require_pidfd_support()
    except RuntimeError as error:
        raise SystemExit(str(error)) from error
    unittest.main()
