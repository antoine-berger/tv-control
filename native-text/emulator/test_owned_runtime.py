#!/usr/bin/env python3
"""Owned host subprocess/mock tests; never execute SDK tools, ADB, or an AVD."""
import io
import contextlib
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
            for value, expected in [("p123\nf5\nn127.0.0.1:49152\n", True),
                                    ("p123\nf5\nn127.0.0.1:49152\nf6\nn[::1]:49152\n", True),
                                    ("p999\nf5\nn127.0.0.1:49152\n", False),
                                    ("p123\nf5\nn*:49152\n", False), ("", False)]:
                with patch.object(owned_runtime, "command", return_value=value) as call:
                    self.assertEqual(owned_runtime.owns_listener(123, 49152, {}), expected)
                    self.assertEqual(call.call_args.args[0], ["/usr/sbin/lsof", "-nP", "-a", "-p", "123",
                                                             "-iTCP:49152", "-sTCP:LISTEN", "-Fpn"])

    def test_darwin_lsof_records_are_complete_unambiguous_and_loopback_only(self):
        valid = "p123\nf5\nn127.0.0.1:49152\n"
        malformed = ["p123\nn127.0.0.1:49152\n",  # old fixture omitted mandatory f
                     valid + "p999\nf6\nn127.0.0.1:49152\n",
                     valid + "p123\nf6\nn127.0.0.1:49152\n",
                     valid + "f5\nn127.0.0.1:49152\n",  # duplicate descriptor
                     valid + "f6\n", valid + "n[::1]:49152\n",
                     valid.replace("f5", "f"), valid.replace("f5", "f05"),
                     valid.replace("f5", "f5u"), valid.replace("f5", "f-1"),
                     valid.replace("f5", "f5\n\n"), valid.replace("f5", "cprocess"),
                     valid.replace("127.0.0.1", "0.0.0.0"),
                     valid.replace("127.0.0.1", "192.0.2.1"),
                     valid.replace("127.0.0.1", "localhost"),
                     valid.replace("49152", "5037"), valid.replace("49152", "15037"),
                     "p123\nf5\nn127.0.0.1:49152\nf6\nn*:49152\n"]
        with patch.object(owned_runtime.sys, "platform", "darwin"):
            for value in malformed:
                with self.subTest(value=value), patch.object(owned_runtime, "command", return_value=value):
                    self.assertFalse(owned_runtime.owns_listener(123, 49152, {}))

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
        self.args = SimpleNamespace(scenario="existing-proof", mode="darwin-local", allow_software_emulation=False,
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
                self.installed = set()
            def start(self):
                events.append("start-adb")
            def target(self, *words, **kwargs):
                events.append(tuple(words))
                if words[-2:] == ("am", "get-current-user"):
                    return "0\n"
                if words[0] == "install":
                    self.installed.add("org.tvcontrol.nativehelper")
                    return "Success\n"
                if "packages" in words:
                    return "package:" + words[-1] + "\n" if words[-1] in self.installed else ""
                if "uninstall" in words:
                    self.installed.discard(words[-1])
                    return "Success\n"
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

    def test_connectivity_scenario_never_calls_old_cases_or_fixture_ui_and_cleans_only_helper(self):
        self.args.scenario = "lab-connectivity"
        fake = SimpleNamespace(pid=456, stdout=io.BytesIO(b""))
        pins = SimpleNamespace(apk=self.build / "lab-helper.apk")
        result = SimpleNamespace(public=lambda: {'status': 'CONNECTED', 'qualified': True})
        with patch.object(run_proof, "clean_source", return_value="a" * 40), \
             patch.object(run_proof.Pins, "load", return_value=pins), \
             patch.object(run_proof, "connectivity", return_value=result) as probe, \
             patch.object(run_proof.subprocess, "Popen", return_value=fake), \
             patch.object(run_proof, "stop_unreaped_group"), patch.object(run_proof, "drain_bounded_log"), \
             patch.object(run_proof, "run_cases") as old:
            run_proof.main()
            old.assert_not_called()
            probe.assert_called_once()
        self.assertIn(("install", "--user", "0", pins.apk), self.events)
        mutations = [event for event in self.events if isinstance(event, tuple) and ("uninstall" in event or "input" in event or "start" in event)]
        self.assertEqual(mutations, [("shell", "-n", "pm", "uninstall", "--user", "0", "org.tvcontrol.nativehelper")])
        self.assertFalse(any(isinstance(event, tuple) and "org.tvcontrol.fixture" in event for event in self.events))
        receipt = json.loads((self.work / "runtime-receipt.json").read_text())
        self.assertTrue(receipt["proof_completed"])
        self.assertTrue(receipt["cleanup_verified"])
        self.assertEqual(receipt["scenario"], "lab-connectivity")

    def test_refused_owned_package_cleanup_is_not_reported_success(self):
        self.args.scenario = "lab-connectivity"
        original = self.fake_adb.target
        def target(adb, *words, **kwargs):
            if "uninstall" in words:
                self.events.append(tuple(words))
                return "Failure [synthetic refusal]\n"
            return original(adb, *words, **kwargs)
        fake = SimpleNamespace(pid=456, stdout=io.BytesIO(b""))
        with patch.object(run_proof, "clean_source", return_value="a" * 40), \
             patch.object(run_proof.Pins, "load", return_value=SimpleNamespace(apk=self.build / "lab-helper.apk")), \
             patch.object(run_proof, "connectivity", return_value=SimpleNamespace(public=lambda: {})), \
             patch.object(self.fake_adb, "target", target), \
             patch.object(run_proof.subprocess, "Popen", return_value=fake), \
             patch.object(run_proof, "stop_unreaped_group"), patch.object(run_proof, "drain_bounded_log"):
            with self.assertRaisesRegex(RuntimeError, "cleanup unverified"):
                run_proof.main()
        receipt = json.loads((self.work / "runtime-receipt.json").read_text())
        self.assertFalse(receipt["cleanup_verified"])
        self.assertIn("owned-package-cleanup", receipt["cleanup_failures"])

    def test_uncertain_lab_stage_propagates_to_final_runtime_cleanup_receipt(self):
        self.args.scenario = "lab-connectivity"
        fake = SimpleNamespace(pid=456, stdout=io.BytesIO(b""))
        with patch.object(run_proof, "clean_source", return_value="a" * 40), \
             patch.object(run_proof.Pins, "load", return_value=SimpleNamespace(apk=self.build / "lab-helper.apk")), \
             patch.object(run_proof, "connectivity", side_effect=run_proof.StageCleanupUnverified), \
             patch.object(run_proof.subprocess, "Popen", return_value=fake), \
             patch.object(run_proof, "stop_unreaped_group"), patch.object(run_proof, "drain_bounded_log"):
            with self.assertRaisesRegex(RuntimeError, "cleanup unverified: lab-stage-cleanup"):
                run_proof.main()
        receipt = json.loads((self.work / "runtime-receipt.json").read_text())
        self.assertFalse(receipt["proof_completed"])
        self.assertFalse(receipt["cleanup_verified"])
        self.assertIn("lab-stage-cleanup", receipt["cleanup_failures"])

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


class ExplicitScenarioTests(unittest.TestCase):
    def test_cli_requires_explicit_scenario_and_never_selects_from_files(self):
        argv = ['run_proof.py', '--mode', 'darwin-local', '--sdk', '/synthetic/sdk', '--build', '/synthetic/build',
                '--work', '/synthetic/new', '--provenance', '/synthetic/receipt']
        with patch.object(sys, 'argv', argv), contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as error:
                run_proof.arguments()
            self.assertEqual(error.exception.code, 2)
        with patch.object(sys, 'argv', argv + ['--scenario', 'lab-connectivity']):
            self.assertEqual(run_proof.arguments().scenario, 'lab-connectivity')
        with patch.object(sys, 'argv', argv + ['--scenario', 'auto']), contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                run_proof.arguments()

    def test_clean_source_does_not_accept_dirty_or_malformed_identity(self):
        with patch.object(run_proof, 'run_command', return_value='?? synthetic-untracked') as command:
            with self.assertRaisesRegex(RuntimeError, 'clean source'):
                run_proof.clean_source()
            self.assertEqual(command.call_count, 1)
        with patch.object(run_proof, 'run_command', side_effect=['', 'bad']):
            with self.assertRaisesRegex(RuntimeError, 'identity'):
                run_proof.clean_source()
        with patch.object(run_proof, 'run_command', side_effect=['', 'a' * 40 + '\n']):
            self.assertEqual(run_proof.clean_source(), 'a' * 40)


if __name__ == "__main__":
    unittest.main()
