#!/usr/bin/env python3
"""Owned host subprocess/mock tests; never execute SDK tools, ADB, or an AVD."""
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import owned_runtime
import run_proof
from runtime_environment import Runtime


class BoundedHostTests(unittest.TestCase):
    def test_success_failure_timeout_and_oversized_output_clean_owned_children(self):
        self.assertEqual(owned_runtime.command([sys.executable, "-c", "print('fixture')"], os.environ, 3), "fixture\n")
        with self.assertRaises(subprocess.CalledProcessError):
            owned_runtime.command([sys.executable, "-c", "raise SystemExit(2)"], os.environ, 3)
        with self.assertRaises(subprocess.TimeoutExpired):
            owned_runtime.command([sys.executable, "-c", "import time; time.sleep(30)"], os.environ, .05)
        with self.assertRaisesRegex(RuntimeError, "output exceeded"):
            owned_runtime.command([sys.executable, "-c", "print('x'*140000)"], os.environ, 3)

    def test_expired_command_never_spawns(self):
        with patch.object(owned_runtime.subprocess, "Popen") as spawn:
            with self.assertRaisesRegex(RuntimeError, "before process dispatch"):
                owned_runtime.command(["never-executed"], {}, 0)
            spawn.assert_not_called()

    def test_target_deadline_is_rechecked_after_listener_and_protocol_guard(self):
        ports = SimpleNamespace(adb_port=49152, console_port=5630)
        adb = owned_runtime.PrivateAdb(Path("/owned/fake-adb"), ports, {})
        adb.deadline = 100
        with patch.object(adb, "guard"), patch.object(owned_runtime.time, "monotonic", side_effect=[90, 101]), \
             patch.object(owned_runtime, "command") as command:
            with self.assertRaisesRegex(RuntimeError, "during preflight"):
                adb.target("install", "/owned/fixture.apk")
            command.assert_not_called()

    def test_environment_does_not_inherit_global_adb_home_keys_or_loader_hooks(self):
        env = owned_runtime.isolated_environment(Path("/private/sdk"), Path("/private/run"), 49152, 5630,
                                                 {"ADB_SERVER_SOCKET": "tcp:foreign:1", "HOME": "/foreign",
                                                  "ANDROID_SERIAL": "foreign", "TOKEN": "fake",
                                                  "DYLD_INSERT_LIBRARIES": "/foreign/inject"})
        self.assertEqual(env["ADB_SERVER_SOCKET"], "tcp:127.0.0.1:49152")
        self.assertEqual(env["ANDROID_ADB_SERVER_PORT"], "49152")
        self.assertEqual(env["ANDROID_SERIAL"], "emulator-5630")
        self.assertEqual(env["ADB_USB"], "0")
        self.assertEqual(env["ADB_EMU"], "0")
        self.assertNotIn("TOKEN", env)
        self.assertNotIn("DYLD_INSERT_LIBRARIES", env)

    def test_darwin_listener_ownership_does_not_accept_wildcard_or_foreign_pid(self):
        with patch.object(owned_runtime.sys, "platform", "darwin"):
            for value, expected in [("p123\nn127.0.0.1:49152\n", True),
                                    ("p123\nn127.0.0.1:49152\nn[::1]:49152\n", True),
                                    ("p999\nn127.0.0.1:49152\n", False),
                                    ("p123\nn*:49152\n", False), ("", False)]:
                with patch.object(owned_runtime, "command", return_value=value):
                    self.assertEqual(owned_runtime.owns_listener(123, 49152, {}), expected)

    def test_log_drain_can_stop_while_an_owned_pipe_writer_remains_open(self):
        read_fd, write_fd = os.pipe()
        source = os.fdopen(read_fd, "rb")
        stop = threading.Event()
        output = io.BytesIO()
        thread = threading.Thread(target=run_proof.drain_bounded_log, args=(source, output, 65536, stop))
        try:
            thread.start()
            os.write(write_fd, b"fixture")
            time.sleep(.02)
            stop.set()
            thread.join(timeout=1)
            self.assertFalse(thread.is_alive(), "an inherited open writer cannot hang teardown")
        finally:
            stop.set()
            thread.join(timeout=1)
            os.close(write_fd)
            source.close()

    def test_linux_sdk_detection_uses_executable_path_not_ps_basename(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            sdk = root / "sdk"
            sdk.mkdir()
            for pid, executable in [("101", str(sdk / "emulator/emulator")), ("102", "/unrelated/emulator")]:
                process = root / "proc" / pid
                process.mkdir(parents=True)
                (process / "status").write_text(f"Uid:\t{os.getuid()}\nPPid:\t1\n")
                (process / "exe").symlink_to(executable)
            self.assertEqual(owned_runtime.linux_sdk_processes(sdk, root / "proc"), [(101, 1)])


class MainCleanupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.lab = Path(self.temp.name)
        self.sdk, self.build, self.work = [self.lab / value for value in ("sdk", "build", "fresh-run")]
        self.sdk.mkdir()
        self.build.mkdir()
        self.args = SimpleNamespace(mode="darwin-local", allow_software_emulation=False,
                                    sdk=self.sdk, build=self.build, work=self.work,
                                    provenance=self.lab / "fake-receipt.json", deadline_seconds=60)
        self.events = []
        events = self.events
        class FakePorts:
            adb_port, console_port = 49152, 5630
            def release_console(self):
                events.append("release-console")
            def close(self):
                events.append("close-ports")
        class FakeAdb:
            process = SimpleNamespace(pid=123)
            def __init__(self, *args):
                pass
            def start(self):
                events.append("start-adb")
            def target(self, *words, **kwargs):
                events.append(tuple(words))
                values = {"sys.boot_completed": "1", "ro.kernel.qemu": "1", "ro.build.version.sdk": "31",
                          "ro.product.cpu.abi": "arm64-v8a"}
                return values.get(words[-1], "Success")
            def close(self):
                events.append("close-adb")
        self.fake_adb = FakeAdb
        self.patches = [patch.object(run_proof, "arguments", return_value=self.args),
                        patch.object(run_proof, "select_runtime", return_value=Runtime("darwin-local", "arm64-v8a", "arm64", "on", 240)),
                        patch.object(run_proof, "verify_local_inputs"), patch.object(run_proof, "Ports", FakePorts),
                        patch.object(run_proof, "PrivateAdb", FakeAdb), patch.object(run_proof, "sdk_processes", return_value=[]),
                        patch.object(run_proof, "port_closed", return_value=True), patch.object(run_proof, "leader_exited", return_value=False),
                        patch.object(run_proof, "owns_listener", return_value=True), patch.object(run_proof, "signal"),
                        patch.object(run_proof, "run_command", side_effect=["1", str(24 * 1024**3), "Hypervisor.Framework ready"])]
        for item in self.patches:
            item.start()
            self.addCleanup(item.stop)

    def test_early_adb_start_failure_still_closes_resources_without_any_emulator(self):
        with patch.object(self.fake_adb, "start", side_effect=RuntimeError("fixture startup failed")), \
             patch.object(run_proof.subprocess, "Popen") as spawn:
            with self.assertRaisesRegex(RuntimeError, "fixture startup failed"):
                run_proof.main()
            spawn.assert_not_called()
        self.assertIn("close-adb", self.events)
        self.assertIn("close-ports", self.events)
        receipt = json.loads((self.work / "runtime-receipt.json").read_text())
        self.assertFalse(receipt["proof_completed"])
        self.assertTrue(receipt["cleanup_verified"])

    def test_interrupt_after_owned_gate_cleans_groups_and_adb_then_propagates(self):
        fake = SimpleNamespace(pid=456, stdout=io.BytesIO(b""))
        with patch.object(run_proof.subprocess, "Popen", return_value=fake), \
             patch.object(run_proof, "stop_unreaped_group", side_effect=lambda *args, **kwargs: self.events.append("stop-emulator")), \
             patch.object(run_proof, "drain_bounded_log"), \
             patch.object(run_proof, "run_cases", side_effect=KeyboardInterrupt):
            with self.assertRaises(KeyboardInterrupt):
                run_proof.main()
        self.assertLess(self.events.index("stop-emulator"), self.events.index("close-adb"))
        receipt = json.loads((self.work / "runtime-receipt.json").read_text())
        self.assertFalse(receipt["proof_completed"])
        self.assertTrue(receipt["cleanup_verified"])

    def test_remaining_listener_is_explicit_failure_without_unowned_pid_signals(self):
        with patch.object(self.fake_adb, "start", side_effect=RuntimeError("fixture startup failed")), \
             patch.object(run_proof, "port_closed", return_value=False), \
             patch.object(run_proof, "stop_unreaped_group") as signals:
            with self.assertRaisesRegex(RuntimeError, "cleanup unverified"):
                run_proof.main()
            signals.assert_not_called()
        self.assertFalse(json.loads((self.work / "runtime-receipt.json").read_text())["cleanup_verified"])

    def test_private_diagnostic_failure_is_not_reported_as_verified_cleanup(self):
        with patch.object(self.fake_adb, "start", side_effect=RuntimeError("fixture startup failed")), \
             patch.object(self.fake_adb, "close", side_effect=RuntimeError("private ADB startup diagnostic preservation failed")), \
             patch.object(run_proof.subprocess, "Popen") as spawn:
            with self.assertRaisesRegex(RuntimeError, "cleanup unverified"):
                run_proof.main()
            spawn.assert_not_called()
        receipt = json.loads((self.work / "runtime-receipt.json").read_text())
        self.assertFalse(receipt["cleanup_verified"])
        self.assertIn("adb-group-or-listener", receipt["cleanup_failures"])
        self.assertIn("close-ports", self.events)


if __name__ == "__main__":
    unittest.main()
