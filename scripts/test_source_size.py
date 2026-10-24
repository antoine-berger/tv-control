#!/usr/bin/env python3
"""Bounded, offline temporary-Git fixtures for the source-size quality gate."""
from contextlib import redirect_stdout
import io
from pathlib import Path
import subprocess
import tempfile
import unittest

from check_source_size import check


class SourceSizeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="source-size-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.git("init", "--quiet")

    def git(self, *arguments):
        return subprocess.run(
            ["git", *arguments], cwd=self.root, check=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10,
        )

    def source(self, name, data, tracked=True):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        if tracked:
            self.git("add", "--", name)
        return path

    def result(self):
        output = io.StringIO()
        with redirect_stdout(output):
            status = check(self.root)
        return status, output.getvalue()

    def test_empty_and_999_lines_pass(self):
        self.source("empty.py", b"")
        self.source("module.rs", b"// line\n" * 999)
        status, output = self.result()
        self.assertEqual(status, 0)
        self.assertIn("2 tracked", output)

    def test_1000_and_1001_lines_fail_in_path_order(self):
        self.source("z.rs", b"\n" * 1000)
        self.source("a space.py", b"# line\n" * 1001)
        status, output = self.result()
        self.assertEqual(status, 1)
        self.assertIn("'z.rs': 1000 lines; must be <1000", output)
        self.assertIn("'a space.py': 1001 lines", output)
        self.assertLess(output.index("a space.py"), output.index("z.rs"))

    def test_unterminated_final_line_counts_and_crlf_is_one_line(self):
        self.source("ok.rs", b"// line\r\n" * 998 + b"// final")
        self.source("bad.py", b"# line\r\n" * 999 + b"# final")
        status, output = self.result()
        self.assertEqual(status, 1)
        self.assertIn("'bad.py': 1000 lines", output)
        self.assertNotIn("ok.rs", output)

    def test_artifacts_and_untracked_files_are_not_sources(self):
        for name in ("generated/bindings.rs", "vendor/pkg/lib.rs", "target/build.py",
                     "dist/build.py", ".venv/helper.py", "__pycache__/cache.py",
                     "Cargo.lock", "notes.md"):
            self.source(name, b"\n" * 1000)
        self.source("untracked.rs", b"\n" * 1000, tracked=False)
        status, output = self.result()
        self.assertEqual(status, 0)
        self.assertIn("0 tracked", output)

    def test_maintained_tests_and_generated_filename_are_checked(self):
        for name in ("tests/fixture.py", "crates/core/src/generated.rs"):
            self.source(name, b"\n" * 1000)
        status, output = self.result()
        self.assertEqual(status, 1)
        self.assertIn("tests/fixture.py", output)
        self.assertIn("crates/core/src/generated.rs", output)

    def test_missing_tracked_source_fails_closed(self):
        self.source("missing.rs", b"").unlink()
        status, output = self.result()
        self.assertEqual(status, 1)
        self.assertIn("'missing.rs': cannot check source (ValueError)", output)

    def test_invalid_utf8_fails_closed(self):
        self.source("bad.py", b"\xff\n")
        status, output = self.result()
        self.assertEqual(status, 1)
        self.assertIn("UnicodeDecodeError", output)

    def test_source_symlink_is_not_followed(self):
        self.source("fixture.txt", b"\n" * 1000)
        (self.root / "link.rs").symlink_to("fixture.txt")
        self.git("add", "link.rs")
        status, output = self.result()
        self.assertEqual(status, 1)
        self.assertIn("'link.rs': cannot check source (ValueError)", output)


if __name__ == "__main__":
    unittest.main()
