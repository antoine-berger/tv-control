#!/usr/bin/env python3
"""Host-only selection/log bounds tests. Does not launch ADB, Android, or an emulator."""
import ast
import io
from pathlib import Path
import unittest
from run_proof import drain_bounded_log, select_acceleration


class RuntimeBackendTests(unittest.TestCase):
    def test_existing_kvm_stays_accelerated(self):
        self.assertEqual(select_acceleration(True, False), ("on", 240))
        self.assertEqual(select_acceleration(True, True), ("on", 240))

    def test_software_requires_explicit_opt_in(self):
        with self.assertRaises(RuntimeError):
            select_acceleration(False, False)
        self.assertEqual(select_acceleration(False, True), ("off", 900))

    def test_log_retention_is_bounded_but_pipe_fully_drained(self):
        data = bytes(range(256)) * 8192
        source, output = io.BytesIO(data), io.BytesIO()
        drain_bounded_log(source, output)
        self.assertEqual(output.getvalue(), data[:64 * 1024])
        self.assertEqual(source.tell(), len(data))

    def test_empty_and_short_logs(self):
        for data in (b"", b"fixture emulator startup"):
            output = io.BytesIO()
            drain_bounded_log(io.BytesIO(data), output)
            self.assertEqual(output.getvalue(), data)

    def test_only_three_owned_installs_receive_bounded_setup_budget(self):
        tree = ast.parse((Path(__file__).resolve().parent / "run_proof.py").read_text())
        installs = []
        for call in ast.walk(tree):
            if not isinstance(call, ast.Call) or not isinstance(call.func, ast.Name):
                continue
            if call.func.id != "command" or not call.args or not isinstance(call.args[0], ast.List):
                continue
            if not any(isinstance(value, ast.Constant) and value.value == "install"
                       for value in call.args[0].elts):
                continue
            installs.append(call)
            timeout = [item.value for item in call.keywords if item.arg == "timeout"]
            self.assertEqual(len(timeout), 1)
            self.assertIsInstance(timeout[0], ast.Constant)
            self.assertEqual(timeout[0].value, 120)
        self.assertEqual(len(installs), 3)

    def test_audio_and_security_boundaries_remain_in_source(self):
        root = Path(__file__).resolve().parent
        runner = (root / "run_proof.py").read_text()
        workflow = (root.parent.parent / ".github/workflows/native-text-runtime.yml").read_text()
        self.assertIn('"-no-audio"', runner)
        self.assertIn('"-accel", acceleration', runner)
        self.assertIn('refusing any existing ADB target/session', runner)
        self.assertIn('contents: read', workflow)
        self.assertIn('persist-credentials: false', workflow)
        for forbidden in ('sudo ', 'chmod ', 'setfacl ', 'usermod ', 'secrets.'):
            self.assertNotIn(forbidden, workflow)
            self.assertNotIn(forbidden, runner)


if __name__ == "__main__":
    unittest.main()
