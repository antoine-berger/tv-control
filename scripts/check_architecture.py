#!/usr/bin/env python3
"""Enforce the dependency direction using Cargo metadata (no dependencies/network)."""
import json
import subprocess
import sys

cargo = sys.argv[1] if len(sys.argv) > 1 else "cargo"
metadata = json.loads(subprocess.check_output([cargo, "metadata", "--no-deps", "--format-version", "1"]))
allowed = {"tv-domain": set(), "tv-application": {"tv-domain"}}
for package in metadata["packages"]:
    if package["name"] in allowed:
        actual = {dep["name"] for dep in package["dependencies"]}
        if not actual <= allowed[package["name"]]:
            raise SystemExit(f"dependency boundary violated: {package['name']}")
print("domain/application dependency boundaries passed")
