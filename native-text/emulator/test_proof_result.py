#!/usr/bin/env python3
"""Diagnostic enum protocol checks; no Android/emulator/device execution."""
from pathlib import Path
import re
import unittest
from unittest.mock import patch
from types import SimpleNamespace
import owned_proof
from proof_result import ALLOWED, STAGES, parse_proof_result, parse_activity_start


def response(proof="failed", stage="REMAP_CHECK", error="ASSERTION", readback="UNCHANGED_LITERAL"):
    return "\n".join("INSTRUMENTATION_RESULT: tv_native_runtime_" + key + "=" + value
                     for key, value in dict(proof=proof, stage=stage, error=error, readback=readback, overlay="NOT_CHECKED", root="NOT_CHECKED", matches="NOT_CHECKED", report_ids="UNKNOWN", interactive="UNKNOWN", keyguard_locked="UNKNOWN", engine_status="NOT_REPORTED", replay_status="NOT_REPORTED").items())


class ProofResultTests(unittest.TestCase):
    def test_pass_requires_complete_success_stage(self):
        self.assertEqual(parse_proof_result(response("passed", "DONE", "NONE", "NOT_CHECKED"))["proof"], "passed")
        for stage, error in (("REMAP_CHECK", "NONE"), ("DONE", "EXCEPTION")):
            with self.assertRaises(RuntimeError):
                parse_proof_result(response("passed", stage, error))

    def test_failure_preserves_only_allowlisted_diagnostics(self):
        for stage in STAGES:
            if stage == "DONE":
                continue
            value = parse_proof_result(response(stage=stage))
            self.assertEqual(value["stage"], stage)
            self.assertEqual(set(value), set(ALLOWED))

    def test_invalid_values_never_echo_input(self):
        private = "not-a-public-enum-private-content"
        for raw in (response(stage=private), response(error=private), response(readback=private)):
            with self.assertRaises(RuntimeError) as error:
                parse_proof_result(raw)
            self.assertNotIn(private, str(error.exception))

    def test_missing_duplicate_unknown_and_failed_instrumentation_refuse(self):
        valid = response()
        cases = [valid.rsplit("\n", 1)[0], valid + "\n" + valid.splitlines()[0],
                 valid + "\nINSTRUMENTATION_RESULT: tv_native_runtime_extra=value",
                 valid + "\nINSTRUMENTATION_FAILED", "x" * (128 * 1024 + 1)]
        for raw in cases:
            with self.assertRaises(RuntimeError):
                parse_proof_result(raw)

    def test_non_protocol_text_is_never_returned(self):
        value = parse_proof_result("arbitrary diagnostic text\n" + response())
        self.assertNotIn("arbitrary", repr(value))

    def test_launch_categories_never_return_arbitrary_text(self):
        good = "Status: ok\nActivity: org.tvcontrol.fixture/.FixtureActivity\n"
        self.assertEqual(parse_activity_start(good), "FIXTURE_OK")
        for raw, expected in (("Error: private", "ERROR"), ("Status: timeout", "NOT_CONFIRMED"),
                              (good + "Status: ok", "NOT_CONFIRMED"),
                              ("Status: ok\nActivity: private", "WRONG_ACTIVITY"),
                              ("x" * (128 * 1024 + 1), "INVALID")):
            self.assertEqual(parse_activity_start(raw), expected)

    def test_new_diagnostic_values_are_strictly_allowlisted(self):
        valid = response()
        for key in ("overlay", "root", "matches", "report_ids", "interactive", "keyguard_locked", "engine_status", "replay_status"):
            prefix = "tv_native_runtime_" + key + "="
            bad = re.sub(prefix + r"[^\n]+", prefix + "private", valid)
            with self.assertRaises(RuntimeError) as error:
                parse_proof_result(bad)
            self.assertNotIn("private", str(error.exception))

    def test_all_engine_status_values_survive_strict_protocol_without_raw_values(self):
        for key in ("engine_status", "replay_status"):
            for status in ALLOWED[key]:
                raw = response(stage="ENGINE_STATUS")
                raw = raw.replace(key + "=NOT_REPORTED", key + "=" + status)
                self.assertEqual(parse_proof_result(raw)[key], status)

    def test_engine_failure_reaches_host_report_as_fixed_enum_without_retry(self):
        calls = []
        def shell(*words, **kwargs):
            calls.append(words)
            if words[:2] == ("settings", "get"):
                return "fixed fixture setting"
            if words[:2] == ("am", "start"):
                return "Status: ok\nActivity: org.tvcontrol.fixture/.FixtureActivity\n"
            if words[:2] == ("am", "instrument"):
                if "ready" in words:
                    return response("passed", "DONE", "NONE", "NOT_CHECKED")
                if "normal" in words:
                    return response(stage="ENGINE_STATUS").replace("engine_status=NOT_REPORTED", "engine_status=REFUSED") + "\nprivate UI text"
                return "tv_native_text_v1=refused"
            return ""
        with patch.object(owned_proof, "MODES", ("normal",)), patch("builtins.print"):
            with self.assertRaisesRegex(RuntimeError, "engine_status=REFUSED replay_status=NOT_REPORTED") as error:
                owned_proof.run_cases(SimpleNamespace(build=Path("/fixture/build")), shell, lambda *args, **kwargs: None)
        self.assertNotIn("private", str(error.exception))
        self.assertEqual(sum("normal" in words for words in calls), 2, "one launch and one engine invocation")

    def test_java_enum_definitions_match_host_allowlist(self):
        source = (Path(__file__).parent / "src/org/tvcontrol/nativeinput/RuntimeProof.java").read_text()
        for java_name, key in (("Stage", "stage"), ("ErrorCode", "error"), ("SyntheticReadback", "readback"), ("RootCategory", "root"), ("Overlay", "overlay"), ("MatchCount", "matches"), ("ObservedState", "interactive")):
            body = re.search(r"enum " + java_name + r" \{([^}]+)\}", source).group(1)
            actual = frozenset(item.strip() for item in body.split(","))
            self.assertEqual(actual, ALLOWED[key])
        self.assertNotIn("getMessage()", source)
        self.assertNotIn("printStackTrace", source)
        observation = (Path(__file__).parent / "src/org/tvcontrol/nativeinput/EngineObservation.java").read_text()
        body = re.search(r"enum Status \{([^}]+)\}", observation).group(1)
        self.assertEqual(frozenset(item.strip() for item in body.split(",")), ALLOWED["engine_status"])
        self.assertEqual(ALLOWED["engine_status"], ALLOWED["replay_status"])


if __name__ == "__main__":
    unittest.main()
