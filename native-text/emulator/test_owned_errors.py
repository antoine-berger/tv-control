#!/usr/bin/env python3
"""Synthetic parser tests only; never obtains real logcat or starts devices."""
from pathlib import Path
import unittest
from owned_errors import summarize_owned_errors


def log(body, pid=41, tag="AndroidRuntime"):
    return f"E/{tag}( {pid}): {body}\n"


class OwnedErrorsTests(unittest.TestCase):
    def test_owned_crash_classes_and_known_frames_only(self):
        raw = log("Process: org.tvcontrol.fixture, PID: 41")
        raw += log("java.lang.RuntimeException: PRIVATE_MESSAGE")
        raw += log("Caused by: java.lang.NullPointerException: PRIVATE_MESSAGE")
        raw += log("    at org.tvcontrol.fixture.FixtureActivity.onCreate(FixtureActivity.java:25)")
        raw += log("    at private.Class.method(Private.java:17)")
        result = summarize_owned_errors(raw, "")
        self.assertEqual(result["crashes"], [{"package": "FIXTURE", "classes": ["java.lang.RuntimeException", "java.lang.NullPointerException"], "frames": ["FixtureActivity.java:25"]}])
        self.assertNotIn("PRIVATE", repr(result))
        self.assertNotIn("private", repr(result))

    def test_unowned_and_mismatched_pid_never_attribute(self):
        for package, pid in (("private.app", 41), ("org.tvcontrol.fixture.extra", 41), ("org.tvcontrol.fixture", 42)):
            raw = log(f"Process: {package}, PID: {pid}") + log("java.lang.RuntimeException: SECRET")
            self.assertEqual(summarize_owned_errors(raw, "")["crashes"], [])

    def test_interleaved_pids_and_new_fatal_reset(self):
        raw = log("Process: org.tvcontrol.nativehelper, PID: 41")
        raw += log("java.lang.SecurityException: SECRET", pid=42)
        raw += log("FATAL EXCEPTION: private_thread")
        raw += log("java.lang.IllegalStateException: SECRET")
        self.assertEqual(summarize_owned_errors(raw, "")["crashes"][0]["classes"], [])

    def test_unknown_classes_are_not_emitted(self):
        raw = log("Process: org.tvcontrol.fixture, PID: 41") + log("secret.CustomException: SECRET")
        self.assertEqual(summarize_owned_errors(raw, "")["crashes"][0]["classes"], ["OTHER_EXCEPTION"])

    def test_frames_exact_and_bounded(self):
        raw = log("Process: org.tvcontrol.nativehelper, PID: 41")
        for n in range(1, 20):
            raw += log(f" at org.tvcontrol.nativeinput.TextEngine.runOnce(TextEngine.java:{n})")
        raw += log(" at org.tvcontrol.fixture.FixtureActivity.onCreate(Wrong.java:2)")
        result = summarize_owned_errors(raw, "")["crashes"][0]["frames"]
        self.assertEqual(len(result), 8)
        self.assertNotIn("Wrong", repr(result))

    def test_owned_anr_exact_package_only(self):
        raw = log("ANR in org.tvcontrol.fixture (SECRET_ACTIVITY)", tag="ActivityManager")
        raw += log("ANR in private.app", tag="ActivityManager")
        raw += log("ANR in org.tvcontrol.nativehelper.extra", tag="ActivityManager")
        self.assertEqual(summarize_owned_errors("", raw)["anrs"], ["FIXTURE"])

    def test_missing_truncated_or_oversized_is_not_invented_evidence(self):
        self.assertEqual(summarize_owned_errors(log("java.lang.RuntimeException: SECRET"), "")["crashes"], [])
        self.assertEqual(summarize_owned_errors("x" * (128 * 1024 + 1), "")["collection"], "OVER_BUDGET")

    def test_collection_and_build_are_diagnostic_only(self):
        root = Path(__file__).parent
        runner = (root / "run_proof.py").read_text()
        self.assertIn('"-d", "-t", "160"', runner)
        self.assertNotIn('"logcat", "-c"', runner)
        self.assertNotIn("print(crash", runner)
        build = (root / "build_runtime.py").read_text()
        self.assertIn('source_lines=name in ("fixture", "proof")', build)


if __name__ == "__main__":
    unittest.main()
