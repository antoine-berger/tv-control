#!/usr/bin/env python3
"""Regression checks: group signals must precede reaping the owned leader."""
import signal
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
        def observe(*args):
            self.assertIsNone(process.returncode, "must not observe reaped PID")
            self.assertTrue(args[2] & process_group.os.WNOWAIT)
            return next(observations)
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


if __name__ == "__main__":
    unittest.main()
