#!/usr/bin/env python3
"""Enforce the project's <1000-line budget for tracked Rust/Python sources."""
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys


LINE_LIMIT = 1000
# Reserved artifact directories, never locations for hand-maintained code.
ARTIFACT_DIRS = {"generated", "vendor", "target", "dist", ".venv", "__pycache__"}


def check(root):
    names = subprocess.check_output(["git", "ls-files", "-z"], cwd=root).split(b"\0")
    checked = 0
    failures = []
    for raw_name in sorted(set(filter(None, names))):
        name = os.fsdecode(raw_name)
        relative = PurePosixPath(name)
        if relative.suffix not in {".rs", ".py"}:
            continue
        if ARTIFACT_DIRS.intersection(relative.parts[:-1]):
            continue
        path = root / name
        try:
            if path.is_symlink() or not path.is_file():
                raise ValueError("expected a tracked regular source file")
            with path.open("rb") as source:
                lines = 0
                for line in source:
                    line.decode("utf-8")
                    lines += 1
            checked += 1
            if lines >= LINE_LIMIT:
                failures.append(f"FAIL {name!r}: {lines} lines; must be <{LINE_LIMIT}")
        except (OSError, UnicodeError, ValueError) as error:
            failures.append(f"FAIL {name!r}: cannot check source ({type(error).__name__})")
    for failure in failures:
        print(failure)
    if failures:
        return 1
    print(f"source-size check passed: {checked} tracked Rust/Python files <{LINE_LIMIT} lines")
    return 0


def main():
    # Anchor to the checkout, not the caller's directory. Tests call check with
    # a temporary Git repository; normal invocations cannot override the budget.
    try:
        return check(Path(__file__).resolve().parent.parent)
    except (OSError, subprocess.CalledProcessError) as error:
        print(f"FAIL source-size inventory: {type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
