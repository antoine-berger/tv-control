#!/usr/bin/env python3
"""Read-only parser/mocks and owned Python children; no SDK server or AVD."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from adb_diagnostic import TAIL_LIMIT, capture, classify
import owned_runtime
from process_group import leader_exited, retained_exit_info, stop_unreaped_group


class DiagnosticTests(unittest.TestCase):
    def test_known_listener_errors_are_fixed_categories_not_arbitrary_output(self):
        for message, expected in [("Operation not permitted", "LISTENER_PERMISSION_DENIED"),
                                  ("Address already in use", "LISTENER_ADDRESS_IN_USE"),
                                  ("listening on specified hostname currently unsupported", "LISTENER_HOSTNAME_UNSUPPORTED")]:
            raw = ("F adb /private/build/client/adb_listeners.cpp:123 could not install *smartsocket* listener: "
                   + message + "\nAuthorization: fake-private-token").encode()
            value = classify(raw, len(raw))
            self.assertEqual(value["categories"], [expected])
            self.assertEqual(value["source_frames"], ["adb_listeners.cpp:123"])
            public = json.dumps(value)
            for private in ("/private", "fake-private-token", "Authorization"):
                self.assertNotIn(private, public)

    def test_unknown_output_never_becomes_error_absence_or_a_raw_preview(self):
        raw = b"UNKNOWN private-key-value secret=fixture password=fixture\xff\n"
        value = classify(raw, len(raw))
        self.assertEqual(value["categories"], ["UNKNOWN"])
        self.assertNotIn("fixture", json.dumps(value))

    def test_tail_is_bounded_and_does_not_change_shared_child_file_offset(self):
        with tempfile.TemporaryFile() as log:
            log.write(b"private-prefix" * 10000 + b"\n-L is incompatible with -H or -P\n")
            log.flush()
            before = log.tell()
            value = capture(log)
            self.assertEqual(value["captured_bytes"], TAIL_LIMIT)
            self.assertTrue(value["truncated"])
            self.assertEqual(value["categories"], ["ARGUMENT_SELECTOR_CONFLICT"])
            self.assertEqual(log.tell(), before)
            self.assertNotIn("private-prefix", json.dumps(value))

    def test_source_frames_are_allowlisted_and_bounded(self):
        raw = ("private.cpp:123 secret\n" + "\n".join(f"main.cpp:{n}" for n in range(20))).encode()
        self.assertEqual(len(classify(raw, len(raw))["source_frames"]), 8)
        self.assertNotIn("private.cpp", json.dumps(classify(raw, len(raw))))

    def test_fake_start_failure_preserves_diagnostic_before_log_close(self):
        ports = SimpleNamespace(adb_port=49152, console_port=5630, release_server=lambda: None)
        env = owned_runtime.isolated_environment(Path("/private/sdk"), Path("/private/work"), 49152, 5630)
        adb = owned_runtime.PrivateAdb(Path("/private/sdk/platform-tools/adb"), ports, env)
        process = SimpleNamespace(pid=123, returncode=None)
        def fake_spawn(argv, **kwargs):
            self.assertEqual(argv, [str(adb.adb), "-L", "tcp:127.0.0.1:49152", "server", "nodaemon"])
            kwargs["stdout"].write(b"adb_listeners.cpp:80 could not install *smartsocket* listener: Operation not permitted\nprivate-key=not-public")
            kwargs["stdout"].flush()
            return process
        with patch.object(owned_runtime, "command", return_value="Android Debug Bridge version 1.0.41\n"), \
             patch.object(owned_runtime.subprocess, "Popen", side_effect=fake_spawn) as spawn, \
             patch.object(owned_runtime, "leader_exited", return_value=True), \
             patch.object(owned_runtime, "retained_exit_info", return_value={"state": "exited", "exit_code": 1}), \
             patch.object(owned_runtime, "stop_unreaped_group"), \
             patch.object(owned_runtime, "port_closed", return_value=True):
            with self.assertRaisesRegex(RuntimeError, "exited before readiness"):
                adb.start()
            self.assertEqual(spawn.call_count, 1, "no startup retry")
            saved = dict(adb.startup_diagnostic)
            self.assertEqual(saved["process"], {"state": "exited", "exit_code": 1})
            self.assertEqual(saved["categories"], ["LISTENER_PERMISSION_DENIED"])
            adb.close()
            self.assertTrue(adb.log.closed)
            self.assertEqual(adb.startup_diagnostic, saved)
            self.assertNotIn("not-public", json.dumps(saved))

    def test_diagnostic_collection_failure_does_not_hide_primary_start_error(self):
        adb = owned_runtime.PrivateAdb(Path("/private/fake"), SimpleNamespace(console_port=5630), {})
        with patch.object(adb, "_start", side_effect=RuntimeError("primary fixture error")), \
             patch.object(owned_runtime, "capture_adb_diagnostic", side_effect=OSError):
            with self.assertRaisesRegex(RuntimeError, "primary fixture error"):
                adb.start()
            self.assertEqual(adb.startup_diagnostic["capture"], "unavailable")

    def test_client_selectors_remain_explicit_without_listen_flag(self):
        ports = SimpleNamespace(adb_port=49152, console_port=5630)
        env = owned_runtime.isolated_environment(Path("/private/sdk"), Path("/private/work"), 49152, 5630)
        adb = owned_runtime.PrivateAdb(Path("/private/fake-adb"), ports, env)
        with patch.object(adb, "guard"), patch.object(owned_runtime, "command", return_value="fixture") as call:
            self.assertEqual(adb.target("shell", "-n", "getprop", "sys.boot_completed"), "fixture")
            argv = call.call_args.args[0]
            self.assertEqual(argv[1:7], ["-H", "127.0.0.1", "-P", "49152", "-s", "emulator-5630"])
            self.assertNotIn("-L", argv)

    def test_exit_info_observes_owned_child_status_without_reaping(self):
        process = subprocess.Popen([sys.executable, "-c", "raise SystemExit(7)"], start_new_session=True)
        try:
            deadline = time.monotonic() + 3
            while not leader_exited(process):
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.01)
            self.assertEqual(retained_exit_info(process), {"state": "exited", "exit_code": 7})
            self.assertIsNone(process.returncode)
            stop_unreaped_group(process, grace_seconds=0)
            self.assertEqual(process.returncode, 7)
        finally:
            if process.returncode is None:
                stop_unreaped_group(process, grace_seconds=0)


if __name__ == "__main__":
    unittest.main()
