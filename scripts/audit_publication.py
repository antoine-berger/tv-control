#!/usr/bin/env python3
"""Fail closed on tracked private paths, address ranges and credential markers.
This is an auditable baseline scan, not a complete secret-detection guarantee.
Matched values are never printed. Review git diff/status before publishing.
"""
import argparse
import pathlib
import re
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("--forbid-literal", action="append", default=[])
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parent.parent
names = subprocess.check_output(["git", "ls-files", "-z"], cwd=root).decode().split("\0")
patterns = {
    "private_key": re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----"),
    "credential_format": re.compile(r"(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{24,})"),
    "personal_home_path": re.compile(r"/(?:Users|home)/[^/\s]+/"),
    "private_ipv4": re.compile(r"(?<![\d.])(?:10(?:\.\d{1,3}){3}|192\.168(?:\.\d{1,3}){2}|172\.(?:1[6-9]|2\d|3[01])(?:\.\d{1,3}){2})(?![\d.])"),
}
violations = []
checked = 0
for name in filter(None, names):
    relative = pathlib.PurePosixPath(name)
    if any(part in {".local", ".toolchain", "target", "captures"} for part in relative.parts) or name.endswith((".local.json", ".pem", ".key")) or relative.name in {".env", "adbkey", "adbkey.pub"}:
        violations.append((name, "private_file"))
        continue
    content = (root / name).read_bytes()
    text = content.decode("utf-8", "replace")
    for rule, pattern in patterns.items():
        if pattern.search(text):
            violations.append((name, rule))
    if any(value and value.lower() in text.lower() for value in args.forbid_literal):
        violations.append((name, "forbidden_literal"))
    checked += 1
for name, rule in violations:
    print(f"FAIL {name}: {rule}")
if violations:
    raise SystemExit(1)
print(f"tracked-content baseline audit passed: {checked} files")
