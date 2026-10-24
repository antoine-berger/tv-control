#!/usr/bin/env python3
"""Pinned official SDK provisioning for disposable CI only; never accepts licenses."""
import argparse
import hashlib
from pathlib import Path, PurePosixPath
import time
import stat
import urllib.request
import zipfile

BASE = "https://dl.google.com/android/repository/"
# Pins from official repository2-1.xml and sys-img/android/sys-img2-3.xml, 2026-10-24.
# SDK metadata currently publishes SHA1 for runtime packages; exact URL+size+digest
# are pinned here. Build archives additionally have locally verified SHA256 pins.
PACKAGES = [
    ("platform-28_r06.zip", 75565084, "9a4e52b1d55bd2e24216b150aafae2503d3efba6", "8452dbbf9668a428abb243c4f02a943b7aa83af3cca627629a15c4c09f28e7bd", "android-9/", "platforms/android-28"),
    ("build-tools_r35_linux.zip", 61958799, "2cfaa0bbb2336e9ec18ed3ecea84fa2e2af607bc", "bd3a4966912eb8b30ed0d00b0cda6b6543b949d5ffe00bea54c04c81e1561d88", "android-15/", "build-tools/35.0.0"),
    ("platform-tools_r37.0.1-linux.zip", 9054187, "477254aa5f903c15cf51001717bdf347fb6b53e0", None, "platform-tools/", "platform-tools"),
    ("emulator-linux_x64-16428233.zip", 349654171, "cd7362ea55dfb86a418958138dc396e74165dd01", None, "emulator/", "emulator"),
    ("sys-img/android/x86_64-31_r05.zip", 657246510, "58bff3cb182c79bbfe3980fe77b87b51b9f7ad71", None, "x86_64/", "system-images/android-31/default/x86_64"),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", required=True, type=Path)
    parser.add_argument("--existing-licenses", required=True, type=Path)
    args = parser.parse_args()
    license_file = args.existing_licenses / "android-sdk-license"
    if not license_file.is_file() or not license_file.read_text().strip():
        raise SystemExit("blocked: runner has no pre-existing Android SDK license acceptance")
    args.sdk.mkdir(parents=True, exist_ok=False)
    for url, size, sha1, sha256, prefix, destination in PACKAGES:
        archive_path = args.sdk / "download.zip"
        start = time.monotonic()
        total = 0
        h1, h256 = hashlib.sha1(), hashlib.sha256()
        with urllib.request.urlopen(BASE + url, timeout=60) as response, archive_path.open("xb") as output:
            if not response.geturl().startswith(BASE):
                raise SystemExit("unexpected SDK download redirect")
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                total += len(chunk)
                if total > size or time.monotonic() - start > 300:
                    raise SystemExit("SDK download exceeds pinned size/deadline")
                h1.update(chunk); h256.update(chunk); output.write(chunk)
        if total != size or h1.hexdigest() != sha1 or (sha256 and h256.hexdigest() != sha256):
            raise SystemExit("SDK pin verification failed: " + url)
        with zipfile.ZipFile(archive_path) as archive:
            for member in archive.infolist():
                if member.is_dir():
                    continue
                name = member.filename
                relative = PurePosixPath(name)
                if not name.startswith(prefix) or relative.is_absolute() or ".." in relative.parts:
                    raise SystemExit("unexpected SDK archive member")
                target = args.sdk / destination / name[len(prefix):]
                target.parent.mkdir(parents=True, exist_ok=True)
                if not target.resolve().is_relative_to(args.sdk.resolve()):
                    raise SystemExit("SDK archive path escapes owned root")
                mode = member.external_attr >> 16
                if stat.S_ISLNK(mode):
                    link = archive.read(member).decode("utf-8")
                    if Path(link).is_absolute() or not (target.parent / link).resolve().is_relative_to(args.sdk.resolve()):
                        raise SystemExit("SDK symlink escapes owned root")
                    target.symlink_to(link)
                elif stat.S_IFMT(mode) in (0, stat.S_IFREG):
                    target.write_bytes(archive.read(member))
                    target.chmod(0o755 if (mode & 0o111) else 0o644)
                else:
                    raise SystemExit("unsupported SDK archive entry type")
        archive_path.unlink()
        print("verified official SDK package:", url, "sha256", h256.hexdigest())


if __name__ == "__main__":
    main()
