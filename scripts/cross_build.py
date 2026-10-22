#!/usr/bin/env python3
"""Cross-build with Rust's bundled lld; never installs to or contacts a router."""
import argparse
import os
import pathlib
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("--cargo", default="cargo")
parser.add_argument("--rustc", default="rustc")
parser.add_argument("--target", default="aarch64-unknown-linux-musl")
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parent.parent
sysroot = pathlib.Path(subprocess.check_output([args.rustc, "--print", "sysroot"], text=True).strip())
host = subprocess.check_output([args.rustc, "-vV"], text=True).split("host: ")[1].splitlines()[0]
linker = sysroot / "lib/rustlib" / host / "bin/rust-lld"
assert linker.is_file(), "bundled rust-lld missing"
assert args.target in {"aarch64-unknown-linux-musl", "x86_64-unknown-linux-musl"}, "unsupported static target"
env = os.environ.copy()
env["RUSTFLAGS"] = f"-C linker={linker} -C link-self-contained=yes -C target-feature=+crt-static --remap-path-prefix={root}=."
subprocess.run([args.cargo, "build", "--locked", "--release", "--target", args.target], cwd=root, env=env, check=True)
subprocess.run(["python3", "scripts/check_elf.py", f"target/{args.target}/release/tv-control", args.target.split("-")[0]], cwd=root, check=True)
