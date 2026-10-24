#!/usr/bin/env python3
"""Regression checks: group signals must precede reaping the owned leader."""
import errno
import signal
import select
import subprocess
import sys
import time
import unittest
from unittest.mock import patch
import process_group


class FakeProcess:
    pid = 12345
    returncode = None
    def __init__(self, events):
        self.events = events
    def wait(self, timeout):
        self.events.append("reap")
        self.returncode = 0
        return 0


class ProcessGroupTests(unittest.TestCase):
    def run_case(self, observations, grace=10):
        events = []
        process = FakeProcess(events)
        last = None
        def observe(*args):
            nonlocal last
            self.assertIsNone(process.returncode, "must not observe reaped PID")
            self.assertTrue(args[2] & process_group.os.WNOWAIT)
            last = next(observations, last)
            return last
        def signal_group(pid, sig):
            self.assertEqual(pid, process.pid)
            self.assertIsNone(process.returncode, "must never signal a reaped/reusable PGID")
            events.append(sig)
        with patch.object(process_group.os, "waitid", side_effect=observe), \
             patch.object(process_group.os, "killpg", side_effect=signal_group), \
             patch.object(process_group.time, "sleep"):
            process_group.stop_unreaped_group(process, grace)
        self.assertEqual(events, [signal.SIGTERM, signal.SIGKILL, "reap"])

    def test_early_launcher_exit_keeps_zombie_until_last_group_signal(self):
        self.run_case(iter([object(), object()]))

    def test_ordinary_exit_reaps_only_after_group_cleanup(self):
        self.run_case(iter([None, None, object()]))

    def test_timeout_kills_before_reaping(self):
        self.run_case(iter([None, None]), grace=0)

    def test_already_reaped_leader_never_signalled(self):
        process = FakeProcess([])
        process.returncode = 0
        with patch.object(process_group.os, "killpg") as kill:
            with self.assertRaisesRegex(RuntimeError, "already reaped"):
                process_group.stop_unreaped_group(process)
            kill.assert_not_called()

    def test_real_exited_child_is_observed_without_reaping(self):
        process = subprocess.Popen([sys.executable, "-c", "pass"], start_new_session=True)
        try:
            deadline = time.monotonic() + 5
            while not process_group.leader_exited(process):
                if time.monotonic() >= deadline:
                    self.fail("owned test child exit deadline")
                time.sleep(0.01)
            self.assertIsNone(process.returncode)
            process_group.stop_unreaped_group(process, grace_seconds=0)
            self.assertEqual(process.returncode, 0)
        finally:
            if process.returncode is None:
                process_group.stop_unreaped_group(process, grace_seconds=0)

    def test_real_running_child_cleanup_is_bounded(self):
        process = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"], start_new_session=True)
        try:
            process_group.stop_unreaped_group(process, grace_seconds=0)
            self.assertIsNotNone(process.returncode)
        finally:
            if process.returncode is None:
                process_group.stop_unreaped_group(process, grace_seconds=0)

    def test_lost_child_ownership_never_signalled(self):
        with patch.object(process_group.os, "waitid", side_effect=ChildProcessError), \
             patch.object(process_group.os, "killpg") as kill:
            with self.assertRaisesRegex(RuntimeError, "ownership lost"):
                process_group.stop_unreaped_group(FakeProcess([]))
            kill.assert_not_called()

    def test_permission_error_requires_complete_retained_singleton_proof(self):
        for exited, singleton in [(False, True), (True, False)]:
            with self.subTest(exited=exited, singleton=singleton), \
                 patch.object(process_group, "leader_exited", return_value=exited), \
                 patch.object(process_group, "singleton_group", return_value=singleton), \
                 patch.object(process_group.os, "killpg", side_effect=PermissionError(errno.EPERM, "test owned group")) as kill:
                with self.assertRaises(PermissionError):
                    process_group.signal_owned_group(FakeProcess([]), signal.SIGTERM)
                self.assertEqual(kill.call_count, 1)

    def test_proved_singleton_zombie_permission_error_does_not_retry_signal(self):
        with patch.object(process_group, "leader_exited", return_value=True), \
             patch.object(process_group, "singleton_group", return_value=True), \
             patch.object(process_group.os, "killpg", side_effect=PermissionError(errno.EPERM, "test owned group")) as kill:
            process_group.signal_owned_group(FakeProcess([]), signal.SIGTERM)
            self.assertEqual(kill.call_count, 1)

    def test_exit_can_become_waitable_after_the_denied_group_signal(self):
        with patch.object(process_group, "leader_exited", side_effect=[False, False, True, True]), \
             patch.object(process_group, "singleton_group", return_value=True), \
             patch.object(process_group.time, "sleep"), \
             patch.object(process_group.os, "killpg", side_effect=PermissionError(errno.EPERM, "test owned group")) as kill:
            process_group.signal_owned_group(FakeProcess([]), signal.SIGTERM)
            self.assertEqual(kill.call_count, 1)

    def test_ownership_lost_during_singleton_proof_keeps_failure(self):
        with patch.object(process_group, "leader_exited", side_effect=[True, True, RuntimeError("lost")]), \
             patch.object(process_group, "singleton_group", return_value=True), \
             patch.object(process_group.os, "killpg", side_effect=PermissionError(errno.EPERM, "test owned group")):
            with self.assertRaisesRegex(RuntimeError, "lost"):
                process_group.signal_owned_group(FakeProcess([]), signal.SIGTERM)

    def test_other_permission_errors_are_not_darwin_zombie_exceptions(self):
        for code in (None, errno.EACCES):
            error = PermissionError() if code is None else PermissionError(code, "fixture")
            with patch.object(process_group, "leader_exited", return_value=True), \
                 patch.object(process_group, "singleton_group", return_value=True), \
                 patch.object(process_group.os, "killpg", side_effect=error):
                with self.assertRaises(PermissionError):
                    process_group.signal_owned_group(FakeProcess([]), signal.SIGTERM)

    @unittest.skipUnless(sys.platform == "darwin", "Darwin public libproc proof")
    def test_real_exit_between_probe_and_denied_signal_uses_fresh_singleton_proof(self):
        process = subprocess.Popen([sys.executable, "-c", "import sys; sys.stdin.read(1)"],
                                   stdin=subprocess.PIPE, start_new_session=True)
        events = []
        def exit_before_permission_error(pid, sig):
            self.assertEqual(pid, process.pid)
            self.assertIsNone(process.returncode)
            if not process.stdin.closed:
                process.stdin.write(b"x")
                process.stdin.close()
                until = time.monotonic() + 3
                while not process_group.leader_exited(process):
                    self.assertLess(time.monotonic(), until)
                    time.sleep(.01)
            events.append(sig)
            raise PermissionError(errno.EPERM, "injected after owned exit")
        try:
            self.assertFalse(process_group.leader_exited(process))
            with patch.object(process_group.os, "killpg", side_effect=exit_before_permission_error):
                process_group.stop_unreaped_group(process)
            self.assertEqual(events, [signal.SIGTERM, signal.SIGKILL])
            self.assertEqual(process.returncode, 0)
        finally:
            if not process.stdin.closed:
                process.stdin.close()
            if process.returncode is None:
                process_group.stop_unreaped_group(process, grace_seconds=0)

    def test_real_group_cleanup_closes_pipe_held_by_a_live_descendant(self):
        child = "import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); print('child-ready',flush=True); time.sleep(30)"
        leader = "import subprocess,sys,time; subprocess.Popen([sys.executable,'-c'," + repr(child) + "]); print('leader-ready',flush=True); time.sleep(30)"
        process = subprocess.Popen([sys.executable, "-c", leader], stdout=subprocess.PIPE,
                                   start_new_session=True, bufsize=0)
        try:
            ready = set()
            while len(ready) < 2:
                self.assertTrue(select.select([process.stdout], [], [], 3)[0])
                ready.add(process.stdout.readline().strip())
            self.assertEqual(ready, {b"child-ready", b"leader-ready"})
            process_group.stop_unreaped_group(process, grace_seconds=0)
            self.assertIsNotNone(process.returncode)
            self.assertTrue(select.select([process.stdout], [], [], 3)[0])
            self.assertEqual(process.stdout.read(), b"", "no descendant may retain the pipe")
        finally:
            if process.returncode is None:
                process_group.stop_unreaped_group(process, grace_seconds=0)
            process.stdout.close()


if __name__ == "__main__":
    unittest.main()
