#!/usr/bin/env python3
"""Offline release guard, provenance, asset, and fail-closed publication tests."""
import hashlib
import importlib.util
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("release", ROOT / "scripts/release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
SHA = "a" * 40
VERSION = "0.4.6"
BRANCH = f"release/v{VERSION}-{SHA}"
REF = "refs/heads/" + BRANCH
SELECTION = {"sha": SHA, "tag": "v" + VERSION, "branch": BRANCH, "run_id": 123, "run_attempt": 1}


def environment():
    return {"GITHUB_SHA": SHA, "GITHUB_ACTIONS": "true", "GITHUB_REPOSITORY": release.REPOSITORY,
            "GITHUB_EVENT_NAME": "push", "GITHUB_REF_TYPE": "branch", "GITHUB_REF": REF,
            "GITHUB_WORKFLOW_REF": f"{release.REPOSITORY}/{release.WORKFLOW}@{REF}",
            "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1", "CHECKS_RESULT": "success",
            "STATIC_LINUX_RESULT": "success"}


def event():
    return {"repository": {"full_name": release.REPOSITORY}, "ref": REF, "after": SHA,
            "deleted": False, "forced": False}


class FakeGitHub:
    def __init__(self):
        self.calls = []
        self.tag_exists = False
        self.existing_release = None
        self.release_listing = []
        self.asset_results = []
        self.fail_upload = False
        self.bad_digest = False
        self.main_sha = SHA
        self.current_sha = SHA
        self.main_passed = True

    def request(self, method, path, payload=None, binary=False, optional=False):
        self.calls.append((method, path, payload))
        suffix = path.removeprefix("/repos/" + release.REPOSITORY)
        if method == "GET" and suffix == "/git/ref/heads/main":
            return {"object": {"type": "commit", "sha": self.main_sha}}
        if method == "GET" and suffix == "/actions/runs/123":
            return {"head_sha": self.current_sha, "head_branch": BRANCH, "event": "push",
                    "path": release.WORKFLOW, "repository": {"full_name": release.REPOSITORY}, "run_attempt": 1}
        if method == "GET" and suffix.startswith("/actions/workflows/ci.yml/runs?"):
            return {"workflow_runs": [{"id": 122, "head_sha": SHA, "head_branch": "main", "event": "push",
                     "path": release.WORKFLOW, "status": "completed", "conclusion": "success",
                     "repository": {"full_name": release.REPOSITORY}}] if self.main_passed else []}
        if method == "GET" and suffix.startswith("/git/ref/tags/"):
            return {"object": {"type": "commit", "sha": SHA}} if self.tag_exists else None
        if method == "GET" and suffix.startswith("/releases/tags/"):
            return self.existing_release
        if method == "GET" and suffix.startswith("/releases?"):
            return self.release_listing
        if method == "POST" and suffix == "/git/refs":
            self.tag_exists = True
            return {"object": {"type": "commit", "sha": payload["sha"]}}
        if method == "POST" and suffix == "/releases":
            return {"id": 456, "draft": True}
        if method == "POST" and suffix.startswith("/releases/456/assets?"):
            if self.fail_upload:
                raise RuntimeError("simulated uncertain upload")
            name = release.urllib.parse.parse_qs(release.urllib.parse.urlparse(path).query)["name"][0]
            asset = {"name": name, "size": len(payload), "state": "uploaded",
                     "digest": "sha256:" + ("0" * 64 if self.bad_digest else hashlib.sha256(payload).hexdigest())}
            self.asset_results.append(asset)
            return asset
        if method == "GET" and suffix == "/releases/456":
            return {"draft": True, "prerelease": True, "tag_name": "v" + VERSION, "assets": self.asset_results}
        if method == "PATCH" and suffix == "/releases/456":
            return {"draft": False, "prerelease": True, "tag_name": "v" + VERSION}
        raise AssertionError(f"unexpected request: {method} {suffix}")


class Guards(unittest.TestCase):
    def test_explicit_sha_bound_selection(self):
        self.assertEqual(release.context(environment(), event(), SHA, VERSION), SELECTION)

    def test_reject_each_invalid_environment(self):
        invalid = {"GITHUB_SHA": "a" * 7, "GITHUB_ACTIONS": "false", "GITHUB_REPOSITORY": "someone/fork",
                   "GITHUB_EVENT_NAME": "pull_request", "GITHUB_REF_TYPE": "tag", "GITHUB_REF": "refs/heads/main",
                   "GITHUB_WORKFLOW_REF": "other", "GITHUB_RUN_ID": "0", "GITHUB_RUN_ATTEMPT": "no",
                   "CHECKS_RESULT": "failure", "STATIC_LINUX_RESULT": "skipped"}
        for key, value in invalid.items():
            with self.subTest(key=key), self.assertRaises(ValueError):
                env = environment()
                env[key] = value
                release.context(env, event(), SHA, VERSION)

    def test_wrong_version_sha_event_or_checkout(self):
        for change in ({"deleted": True}, {"forced": True}, {"after": "b" * 40}, {"ref": "refs/heads/main"},
                       {"repository": {"full_name": "someone/fork"}}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                release.context(environment(), {**event(), **change}, SHA, VERSION)
        for head, version in (("b" * 40, VERSION), (SHA, "0.5.0"), (SHA, "0.4.6-rc1")):
            with self.subTest(head=head, version=version), self.assertRaises(ValueError):
                release.context(environment(), event(), head, version)


class Assets(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="tv-release-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.artifacts = self.root / "artifacts"
        for artifact, name in release.PACKAGES.items():
            binary = bytearray(64)
            if "darwin" in name:
                binary[:4] = b"\xcf\xfa\xed\xfe"
                struct.pack_into("<I", binary, 4, 0x0100000C)
            else:
                binary[:6] = b"\x7fELF\x02\x01"
                struct.pack_into("<H", binary, 18, 183 if "aarch64" in name else 62)
            executable = self.root / "fake-binary"
            executable.write_bytes(binary)
            subprocess.run([sys.executable, str(ROOT / "scripts/package.py"), str(executable),
                            str(self.artifacts / artifact / name)], check=True, capture_output=True)

    def test_exact_eight_assets(self):
        self.assertEqual(len(release.assets(self.artifacts)), 8)

    def test_checksum_mismatch(self):
        folder, name = next(iter(release.PACKAGES.items()))
        path = self.artifacts / folder / name
        path.write_bytes(path.read_bytes() + b"tampered")
        with self.assertRaises(subprocess.CalledProcessError):
            release.assets(self.artifacts)

    def test_extra_artifact_or_file(self):
        extra = self.artifacts / "unexpected"
        extra.mkdir()
        with self.assertRaises(ValueError):
            release.assets(self.artifacts)
        extra.rmdir()
        (self.artifacts / next(iter(release.PACKAGES)) / "extra.txt").write_text("extra")
        with self.assertRaises(ValueError):
            release.assets(self.artifacts)

    def test_linked_file_rejected(self):
        folder, name = next(iter(release.PACKAGES.items()))
        path = self.artifacts / folder / name
        saved = self.root / "saved"
        path.rename(saved)
        path.symlink_to(saved)
        with self.assertRaises(ValueError):
            release.assets(self.artifacts)

    def test_bundled_source_must_match_commit(self):
        real_root = release.ROOT
        fake_root = self.root / "source"
        fake_root.mkdir()
        # Keep the real verifier; make the first bundled documentation file differ.
        (fake_root / "scripts").symlink_to(real_root / "scripts", target_is_directory=True)
        for member in ("LICENSE", "README.md", "install.sh"):
            (fake_root / member).write_bytes((real_root / member).read_bytes() + b"changed")
        with patch.object(release, "ROOT", fake_root), self.assertRaises(ValueError):
            release.assets(self.artifacts)


class Publication(unittest.TestCase):
    def setUp(self):
        self.api = FakeGitHub()
        self.files = {f"asset-{n}": str(n).encode() for n in range(8)}

    def writes(self):
        return [call for call in self.api.calls if call[0] != "GET"]

    def test_draft_upload_verify_then_nonlatest_prerelease(self):
        result = release.publish(self.api, SELECTION, self.files)
        self.assertTrue(result["prerelease"])
        self.assertFalse(result["promoted"])
        writes = self.writes()
        self.assertEqual(writes[0][2], {"ref": "refs/tags/v0.4.6", "sha": SHA})
        self.assertEqual(writes[1][2]["make_latest"], "false")
        self.assertTrue(writes[1][2]["draft"])
        self.assertEqual(writes[-1][2], {"draft": False, "prerelease": True, "make_latest": "false"})
        self.assertEqual(sum("/assets?" in path for _, path, _ in writes), 8)
        self.assertFalse(any(method == "DELETE" for method, _, _ in writes))

    def test_existing_tag_release_or_draft_stops_before_writes(self):
        for attribute, value in (("tag_exists", True), ("existing_release", {"id": 9}),
                                 ("release_listing", [{"tag_name": "v0.4.6", "draft": True}])):
            with self.subTest(attribute=attribute):
                self.api = FakeGitHub()
                setattr(self.api, attribute, value)
                with self.assertRaises(ValueError):
                    release.publish(self.api, SELECTION, self.files)
                self.assertEqual(self.writes(), [])

    def test_wrong_main_run_or_commit_stops_before_writes(self):
        for attribute, value in (("main_sha", "b" * 40), ("current_sha", "b" * 40), ("main_passed", False)):
            with self.subTest(attribute=attribute):
                self.api = FakeGitHub()
                setattr(self.api, attribute, value)
                with self.assertRaises(ValueError):
                    release.publish(self.api, SELECTION, self.files)
                self.assertEqual(self.writes(), [])

    def test_upload_failure_or_bad_digest_never_publishes_or_retries(self):
        for attribute in ("fail_upload", "bad_digest"):
            with self.subTest(attribute=attribute):
                self.api = FakeGitHub()
                setattr(self.api, attribute, True)
                with self.assertRaises((ValueError, RuntimeError)):
                    release.publish(self.api, SELECTION, self.files)
                self.assertEqual(sum("/assets?" in path for _, path, _ in self.writes()), 1)
                self.assertFalse(any(method in ("PATCH", "DELETE") for method, _, _ in self.writes()))

    def test_api_rejects_other_repository_before_network(self):
        with self.assertRaises(ValueError):
            release.GitHub("test-only").request("POST", "/repos/other/repo/releases", {})


if __name__ == "__main__":
    unittest.main()
