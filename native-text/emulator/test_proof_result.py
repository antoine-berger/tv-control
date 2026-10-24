#!/usr/bin/env python3
"""Diagnostic enum protocol checks; no Android/emulator/device execution."""
from pathlib import Path
import re
import unittest
from proof_result import ALLOWED, STAGES, parse_proof_result


def response(proof="failed", stage="REMAP_CHECK", error="ASSERTION", readback="UNCHANGED_LITERAL"):
    return "\n".join("INSTRUMENTATION_RESULT: tv_native_runtime_" + key + "=" + value
                     for key, value in dict(proof=proof, stage=stage, error=error, readback=readback).items())


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

    def test_java_enum_definitions_match_host_allowlist(self):
        source = (Path(__file__).parent / "src/org/tvcontrol/nativeinput/RuntimeProof.java").read_text()
        for java_name, key in (("Stage", "stage"), ("ErrorCode", "error"), ("SyntheticReadback", "readback")):
            body = re.search(r"enum " + java_name + r" \{([^}]+)\}", source).group(1)
            actual = frozenset(item.strip() for item in body.split(","))
            self.assertEqual(actual, ALLOWED[key])
        self.assertNotIn("getMessage()", source)
        self.assertNotIn("printStackTrace", source)


if __name__ == "__main__":
    unittest.main()
