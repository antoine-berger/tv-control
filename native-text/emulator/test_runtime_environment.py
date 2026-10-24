#!/usr/bin/env python3
"""SDK/ABI/provenance tests use fake files only. No tool or emulator executes."""
import hashlib
import json
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

from runtime_environment import avd_config, macho_arm64, select_runtime, verify_local_inputs


class RuntimeEnvironmentTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.lab = Path(self.temp.name)
        self.lab.chmod(0o700)
        self.sdk = self.lab / "sdk"
        self.sdk.mkdir()
        self.build = self.lab / "build"
        self.build.mkdir()
        self.work = self.lab / "new-avd"
        self.receipt = self.lab / "provenance.json"
        self.runtime = select_runtime("darwin-local", False, system="Darwin", machine="arm64")
        image = self.runtime.image
        header = b"\xcf\xfa\xed\xfe" + struct.pack("<I", 0x0100000C) + bytes(24)
        files = {"platform-tools/adb": header, "emulator/emulator": header,
                 "licenses/android-sdk-license": b"fixture accepted stable license\n",
                 image + "/source.properties": b"AndroidVersion.ApiLevel=31\nSystemImage.Abi=arm64-v8a\nSystemImage.TagId=default\nPkg.Revision=3\n"}
        files.update({image + "/" + name: b"owned fake image bytes" for name in
                      ("kernel-ranchu", "ramdisk.img", "system.img", "vendor.img", "userdata.img")})
        hashes = {}
        for name, data in files.items():
            path = self.sdk / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            hashes[name] = hashlib.sha256(data).hexdigest()
        self.metadata = {"schema": 1, "sdk_root": str(self.sdk.resolve()), "files": hashes,
                         "packages": [{"sdk_path": path, "archive_url": "https://dl.google.com/android/repository/fixture.zip",
                                       "published_checksum": {"algorithm": "sha1", "value": "0" * 40},
                                       "archive_size": 1, "archive_sha256": "0" * 64, "verified": True}
                                      for path in ("platform-tools", "emulator", image)],
                         "licenses": {"selected_google_id": "android-sdk-license", "preview_selected": False, "analytics_opted_in": False},
                         "repo_revision": 4, "source_revision": 3}
        artifacts = {}
        for name in ("fixture.apk", "helper.apk", "proof.apk"):
            path = self.build / name
            path.write_bytes(b"owned fake APK bytes")
            artifacts[name] = hashlib.sha256(path.read_bytes()).hexdigest()
        (self.build / "build-receipt.json").write_text(json.dumps({"schema": 1, "artifacts": artifacts}))
        self.save()

    def save(self):
        self.receipt.write_text(json.dumps(self.metadata))

    def verify(self):
        return verify_local_inputs(self.sdk, self.build, self.work, self.receipt, self.runtime)

    def test_arm64_local_mode_is_explicit_and_never_uses_software_or_fake_ci(self):
        for system, machine, software in [("Linux", "arm64", False), ("Darwin", "x86_64", False), ("Darwin", "arm64", True)]:
            with self.assertRaises(RuntimeError):
                select_runtime("darwin-local", software, system=system, machine=machine)
        self.assertEqual(self.runtime.acceleration, "on")
        self.assertEqual(self.runtime.boot_seconds, 240)
        with self.assertRaises(RuntimeError):
            select_runtime("ci-linux", True, system="Linux", machine="x86_64", environ={})
        ci = select_runtime("ci-linux", True, system="Linux", machine="x86_64",
                            environ={"GITHUB_ACTIONS": "true", "TV_NATIVE_OWNED_AVD": "api31-disposable"})
        self.assertEqual((ci.acceleration, ci.abi, ci.boot_seconds), ("off", "x86_64", 900))

    def test_exact_verified_image_accepts_distinct_recorded_metadata_revisions(self):
        self.assertEqual(self.verify()["source_revision"], 3)
        self.assertFalse(self.work.exists(), "preflight cannot create runtime state")
        config = avd_config(self.runtime)
        for required in ("abi.type=arm64-v8a", "hw.cpu.arch=arm64", "hw.audioOutput=no", "hw.audioInput=no"):
            self.assertIn(required, config)
        self.assertNotIn("x86_64", config)

    def test_modified_binary_and_non_arm64_executable_fail(self):
        path = self.sdk / "emulator/emulator"
        path.write_bytes(b"changed")
        with self.assertRaisesRegex(RuntimeError, "changed"):
            self.verify()
        self.metadata["files"]["emulator/emulator"] = hashlib.sha256(path.read_bytes()).hexdigest()
        self.save()
        with self.assertRaisesRegex(RuntimeError, "ARM64"):
            self.verify()

    def test_unlisted_runtime_file_and_mismatching_image_abi_fail(self):
        extra = self.sdk / "emulator/unverified-library"
        extra.write_bytes(b"extra")
        with self.assertRaisesRegex(RuntimeError, "outside verified provenance"):
            self.verify()
        extra.unlink()  # This test's own newly created fake file only.
        name = self.runtime.image + "/source.properties"
        source = self.sdk / name
        source.write_text(source.read_text().replace("arm64-v8a", "x86_64"))
        self.metadata["files"][name] = hashlib.sha256(source.read_bytes()).hexdigest()
        self.save()
        with self.assertRaisesRegex(RuntimeError, "exact AOSP"):
            self.verify()

    def test_empty_missing_or_preview_license_is_refused_without_acceptance(self):
        license_path = self.sdk / "licenses/android-sdk-license"
        license_path.write_text("")
        with self.assertRaisesRegex(RuntimeError, "acceptance"):
            self.verify()
        self.metadata["licenses"]["preview_selected"] = True
        self.save()
        with self.assertRaisesRegex(RuntimeError, "stable packages"):
            self.verify()

    def test_missing_unverified_or_nonofficial_package_provenance_fails(self):
        self.metadata["packages"][0]["verified"] = False
        self.save()
        with self.assertRaisesRegex(RuntimeError, "provenance"):
            self.verify()
        self.metadata["packages"][0]["verified"] = True
        self.metadata["packages"][0]["archive_url"] = "https://example.invalid/fixture.zip"
        self.save()
        with self.assertRaisesRegex(RuntimeError, "provenance"):
            self.verify()

    def test_sdk_symlink_outside_lab_cannot_be_rebound_by_receipt(self):
        with tempfile.TemporaryDirectory() as elsewhere:
            original = self.sdk
            self.sdk = self.lab / "linked-sdk"
            self.sdk.symlink_to(elsewhere, target_is_directory=True)
            self.metadata["sdk_root"] = str(Path(elsewhere).resolve())
            self.save()
            with self.assertRaisesRegex(RuntimeError, "SDK root"):
                self.verify()
            self.sdk = original

    def test_control_character_existing_work_and_modified_apk_fail(self):
        self.work = self.lab / "bad\npath"
        with self.assertRaisesRegex(RuntimeError, "control characters"):
            self.verify()
        self.work = self.lab / "existing"
        self.work.mkdir()
        with self.assertRaisesRegex(RuntimeError, "must be new"):
            self.verify()
        self.work = self.lab / "fresh"
        (self.build / "proof.apk").write_bytes(b"changed")
        with self.assertRaisesRegex(RuntimeError, "APK changed"):
            self.verify()

    def test_no_nonreaping_api_or_insufficient_disk_fails_before_state_creation(self):
        with patch("runtime_environment.require_nonreaping_wait", side_effect=RuntimeError("waitid missing")):
            with self.assertRaisesRegex(RuntimeError, "waitid"):
                self.verify()
        with patch("runtime_environment.shutil.disk_usage", return_value=type("Disk", (), {"free": 1})()):
            with self.assertRaisesRegex(RuntimeError, "free"):
                self.verify()
        self.assertFalse(self.work.exists())


if __name__ == "__main__":
    unittest.main()
