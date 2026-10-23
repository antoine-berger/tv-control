#!/usr/bin/env python3
"""Complete offline pipeline contracts. No TV, account, TTY or child controller."""
import json
import os
import pathlib
import shlex
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
BINARY = pathlib.Path(sys.argv.pop(1)).resolve() if len(sys.argv) > 1 else ROOT / "target/debug/tv-control"
STAGES = ["configuration", "adb_consent", "adb_runtime", "device_observation",
          "local_mcp", "tunnel_consent", "tunnel_runtime", "external_conversation"]


class JourneyFixture(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="journey fixture's ")
        self.dir = pathlib.Path(self.tmp.name)
        self.marker = self.dir / "unexpected-child"
        self.bin = self.dir / "binaries"
        self.bin.mkdir()
        for name in ["adb", "curl", "tunnel-client"]:
            executable = self.bin / name
            executable.write_text("#!/bin/sh\nprintf child > " + shlex.quote(str(self.marker)) + "\nexit 99\n")
            executable.chmod(0o700)
        config = json.loads((ROOT / "examples/config.json").read_text())
        config["executables"] = {"adb": str(self.bin / "adb"), "curl": str(self.bin / "curl")}
        config["devices"]["private_fixture_alias"] = config["devices"].pop("screen")
        self.config = self.dir / "configuration.json"
        self.config.write_text(json.dumps(config))
        self.fixture = self.dir / "fixture.json"
        self.env = dict(os.environ, HOME=str(self.dir / "home"), PATH=str(self.bin),
                        XDG_CONFIG_HOME=str(self.dir / "xdg"),
                        TV_LIBRARY_TOKEN="private_fixture_secret",
                        TV_CONTROL_TUNNEL_KEY="private_fixture_secret")

    def tearDown(self):
        self.tmp.cleanup()

    def run_fixture(self, fixture, raw=False):
        self.fixture.write_text(fixture if raw else json.dumps(fixture))
        before = {str(p.relative_to(self.dir)): p.read_bytes()
                  for p in self.dir.rglob("*") if p.is_file()}
        result = subprocess.run([str(BINARY), "onboard", str(self.config), "--fixture", str(self.fixture), "--json"],
                                input="", capture_output=True, text=True, env=self.env, timeout=5)
        after = {str(p.relative_to(self.dir)): p.read_bytes()
                 for p in self.dir.rglob("*") if p.is_file()}
        self.assertEqual(before, after, "fixture path changed local files")
        self.assertFalse(self.marker.exists(), "fixture path started a native controller")
        self.assertNotIn("private_fixture_secret", result.stdout + result.stderr)
        self.assertNotIn("private_fixture_alias", result.stdout + result.stderr)
        self.assertEqual(len(result.stdout.splitlines()), 1, result.stdout)
        result.value = json.loads(result.stdout)
        self.assertIsNot(result.value.get("ready"), True)
        return result

    def test_complete_pipeline_is_simulated_and_never_ready(self):
        result = self.run_fixture({"schema": 1})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.value, {"status": "simulation_complete", "mode": "simulated",
                                       "ready": False, "stages": STAGES})
        self.assertEqual(result.stderr, "")

    def test_every_stage_and_stop_reason_halts_before_following_stages(self):
        for index, stage in enumerate(STAGES):
            for reason in ["pending", "rejected", "cancelled", "expired"]:
                with self.subTest(stage=stage, reason=reason):
                    result = self.run_fixture({"schema": 1, "stop_stage": stage, "reason": reason})
                    self.assertEqual(result.returncode, 2, result.stderr)
                    self.assertEqual(result.value, {"status": "paused", "mode": "simulated", "ready": False,
                                                   "stage": stage, "reason": reason, "stages": STAGES[:index + 1]})
                    self.assertEqual(result.stderr, "")

    def test_reason_defaults_to_pending(self):
        result = self.run_fixture({"schema": 1, "stop_stage": "external_conversation"})
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.value["reason"], "pending")

    def test_unknown_fields_modes_reasons_and_schemas_are_rejected(self):
        for fixture in [{"schema": 1, "mode": "live"}, {"schema": 1, "unknown": 1},
                        {"schema": 1, "stop_stage": "ready"},
                        {"schema": 1, "stop_stage": "local_mcp", "reason": "approved"},
                        {"schema": 1, "reason": "pending"}, {"schema": 2}, {}, []]:
            with self.subTest(fixture=fixture):
                result = self.run_fixture(fixture)
                self.assertEqual(result.returncode, 1)
                self.assertEqual(result.value["code"], "invalid_journey_fixture")

    def test_truncated_duplicate_and_oversized_fixtures_are_rejected(self):
        for raw in ['{"schema":', '{"schema":1,"schema":1}', ' ' * 16385]:
            with self.subTest(size=len(raw)):
                result = self.run_fixture(raw, raw=True)
                self.assertEqual(result.returncode, 1)
                self.assertIn(result.value["code"], ["invalid_journey_fixture", "fixture_input_too_large"])

    def test_invalid_or_oversized_configuration_has_no_effects(self):
        for raw in ["{broken", " " * 1048577]:
            with self.subTest(size=len(raw)):
                self.config.write_text(raw)
                result = self.run_fixture({"schema": 1})
                self.assertEqual(result.returncode, 1)
                self.assertIn(result.value["code"], ["invalid_configuration_json", "fixture_input_too_large"])

    def test_jellyfin_only_config_cannot_silently_simulate_adb(self):
        config = json.loads(self.config.read_text())
        del config["devices"]["private_fixture_alias"]
        self.config.write_text(json.dumps(config))
        result = self.run_fixture({"schema": 1})
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.value["code"], "fixture_requires_adb_configuration")


if __name__ == "__main__":
    unittest.main()
