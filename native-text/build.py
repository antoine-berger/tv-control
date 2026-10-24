#!/usr/bin/env python3
"""Offline deterministic helper build. No download, deployment, adb or installation."""
import argparse
import hashlib
from pathlib import Path
import subprocess
import tempfile
import zipfile

PINS = {
    "platform": "8452dbbf9668a428abb243c4f02a943b7aa83af3cca627629a15c4c09f28e7bd",
    "tools": "bd3a4966912eb8b30ed0d00b0cda6b6543b949d5ffe00bea54c04c81e1561d88",
}
COMPILER = "javac 21.0.12.1"


def checked_archive(path, kind):
    if hashlib.sha256(path.read_bytes()).hexdigest() != PINS[kind]:
        raise SystemExit(f"{kind} archive SHA256 mismatch")
    return zipfile.ZipFile(path)


def extract(archive, name, root):
    # Exact project-owned member list only, no extractall/traversal/symlinks.
    destination = root / Path(name).name
    destination.write_bytes(archive.read(name))
    return str(destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", required=True, type=Path)
    parser.add_argument("--tools", required=True, type=Path)
    parser.add_argument("--java", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    compiler = [args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main"]
    actual = subprocess.check_output(compiler + ["-version"], text=True).strip()
    if actual != COMPILER:
        raise SystemExit(f"Compiler mismatch: expected {COMPILER}; got {actual}")
    with checked_archive(args.platform, "platform") as platform, \
            checked_archive(args.tools, "tools") as tools, \
            tempfile.TemporaryDirectory(prefix="tv-native-build-") as temporary:
        root = Path(temporary)
        android = extract(platform, "android-9/android.jar", root)
        automator = extract(platform, "android-9/uiautomator.jar", root)
        junit = extract(platform, "android-9/optional/android.test.base.jar", root)
        d8 = extract(tools, "android-15/lib/d8.jar", root)
        classes, dex = root / "classes", root / "dex"
        classes.mkdir(); dex.mkdir()
        sources = sorted((Path(__file__).parent / "src/org/tvcontrol/nativeinput").glob("*.java"))
        subprocess.run(compiler + ["-source", "8", "-target", "8", "-Xlint:-options",
            "-encoding", "UTF-8", "-g:none", "-bootclasspath", android,
            "-cp", automator + ":" + junit, "-d", str(classes), *[str(source) for source in sources]], check=True)
        subprocess.run([args.java, "-cp", d8, "com.android.tools.r8.D8", "--release",
            "--min-api", "26", "--lib", android, "--classpath", automator,
            "--classpath", junit, "--output", str(dex)] +
            [str(p) for p in sorted(classes.rglob("*.class"))], check=True)
        payload = (dex / "classes.dex").read_bytes()
        if not payload.startswith(b"dex\n"):
            raise SystemExit("Compiler did not produce DEX")
        entry = zipfile.ZipInfo("classes.dex", date_time=(1980, 1, 1, 0, 0, 0))
        entry.external_attr = 0o100644 << 16
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(args.output, "w", compression=zipfile.ZIP_STORED) as jar:
            jar.writestr(entry, payload)
        print("sha256", hashlib.sha256(args.output.read_bytes()).hexdigest())


if __name__ == "__main__":
    main()
