#!/usr/bin/env python3
"""Offline public-verification selection and no-write regression tests."""
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import unittest
from unittest.mock import patch

import release
import test_release
import verify_public_release as verify

SHA = test_release.SHA
TAG = "v0.3.0"


class PublicAPI:
    def __init__(self):
        self.calls = []
        self.files = {name: name.encode() for name in verify.ASSET_NAMES}
        hashes = {name: hashlib.sha256(data).hexdigest() for name, data in self.files.items()}
        body = "\n".join(["Source commit: " + SHA,
            f"Main CI: https://github.com/{release.REPOSITORY}/actions/runs/122",
            f"Build and package CI: https://github.com/{release.REPOSITORY}/actions/runs/123",
            *[f"{digest}  {name}" for name, digest in sorted(hashes.items())]])
        self.item = {"id": 456, "tag_name": TAG, "draft": False, "prerelease": True, "body": body,
                     "assets": [{"name": name, "digest": "sha256:" + hashes[name], "size": len(data),
                        "state": "uploaded", "browser_download_url":
                        f"https://github.com/{release.REPOSITORY}/releases/download/{TAG}/{name}"}
                        for name, data in self.files.items()]}
        self.tag_sha = SHA
        self.ci_conclusion = "success"

    def request(self, method, path, **kwargs):
        self.calls.append((method, path))
        if method != "GET":
            raise AssertionError("public verifier must never write")
        if "/releases/" in path:
            return copy.deepcopy(self.item)
        if "/git/ref/tags/" in path:
            return {"object": {"type": "commit", "sha": self.tag_sha}}
        if "/actions/runs/" in path:
            branch = "main" if path.endswith("/122") else f"release/{TAG}-{SHA}"
            return {"head_sha": SHA, "head_branch": branch, "event": "push", "path": release.WORKFLOW,
                    "status": "completed", "conclusion": self.ci_conclusion,
                    "repository": {"full_name": release.REPOSITORY}}
        raise AssertionError(path)


class Candidate(unittest.TestCase):
    def setUp(self):
        self.api = PublicAPI()

    def test_manifest_binds_source_release_id_and_all_hashes(self):
        manifest, urls = verify.candidate(self.api, 456, SHA, TAG)
        self.assertEqual(set(urls), verify.ASSET_NAMES)
        digest = verify.manifest_digest(manifest)
        for key, value in (("release_id", 457), ("source_sha", "b" * 40), ("tag", "v0.4.0")):
            changed = {**manifest, key: value}
            self.assertNotEqual(verify.manifest_digest(changed), digest)
        changed = copy.deepcopy(manifest)
        changed["asset_sha256"]["install.sh"] = "0" * 64
        self.assertNotEqual(verify.manifest_digest(changed), digest)
        self.assertTrue(all(method == "GET" for method, _ in self.api.calls))

    def test_wrong_release_id_and_changed_metadata_rejected(self):
        for key, value in (("id", 999), ("tag_name", "v0.4.0"), ("draft", True), ("prerelease", False)):
            with self.subTest(key=key):
                self.api = PublicAPI()
                self.api.item[key] = value
                with self.assertRaises(ValueError):
                    verify.candidate(self.api, 456, SHA, TAG)
                self.assertTrue(all(method == "GET" for method, _ in self.api.calls))

    def test_failed_original_matrix_or_changed_tag_never_certified(self):
        for key, value in (("ci_conclusion", "failure"), ("tag_sha", "b" * 40)):
            with self.subTest(key=key):
                self.api = PublicAPI()
                setattr(self.api, key, value)
                with self.assertRaises(ValueError):
                    verify.candidate(self.api, 456, SHA, TAG)
                self.assertTrue(all(method == "GET" for method, _ in self.api.calls))

    def test_tampered_asset_digest_url_or_inventory_rejected(self):
        for key, value in (("digest", "sha256:" + "0" * 64), ("browser_download_url", "https://example.invalid/file"),
                           ("state", "starter"), ("size", 0)):
            with self.subTest(key=key):
                self.api = PublicAPI()
                self.api.item["assets"][0][key] = value
                with self.assertRaises(ValueError):
                    verify.candidate(self.api, 456, SHA, TAG)
        self.api = PublicAPI()
        self.api.item["assets"].pop()
        with self.assertRaises(ValueError):
            verify.candidate(self.api, 456, SHA, TAG)

    def test_exact_verification_context_cannot_publish(self):
        manifest, _ = verify.candidate(self.api, 456, SHA, TAG)
        digest = verify.manifest_digest(manifest)
        branch = f"verify/{TAG}-{SHA}-r456-{digest}"
        env, event = test_release.environment(), test_release.event()
        env["GITHUB_REF"] = event["ref"] = "refs/heads/" + branch
        env["GITHUB_WORKFLOW_REF"] = f"{release.REPOSITORY}/{release.WORKFLOW}@{event['ref']}"
        selected = verify.context(env, event, SHA, "0.3.0")
        self.assertEqual(selected["release_id"], 456)
        self.assertEqual(selected["manifest_sha256"], digest)
        with self.assertRaises(ValueError):
            release.context(env, event, SHA, "0.3.0")
        env["GITHUB_REF"] += "extra"
        with self.assertRaises(ValueError):
            verify.context(env, event, SHA, "0.3.0")

    def test_read_only_client_refuses_writes_before_network(self):
        with self.assertRaises(ValueError):
            release.GitHub(anonymous=True).request("PATCH", f"/repos/{release.REPOSITORY}/releases/456", {})


class Downloads(unittest.TestCase):
    def setUp(self):
        fixture = test_release.Assets(methodName="test_exact_eight_assets")
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.root = fixture.root
        self.files = release.assets(fixture.artifacts)
        self.manifest = {"release_id": 456, "tag": TAG, "source_sha": SHA,
                         "asset_sha256": {name: hashlib.sha256(data).hexdigest() for name, data in self.files.items()}}
        self.urls = {name: name for name in self.files}

    def test_redownloaded_bytes_all_validate(self):
        result = verify.download_assets(self.manifest, self.urls, self.root / "fresh", self.files.__getitem__)
        self.assertEqual(result, self.files)

    def test_tampered_public_download_fails_before_execution(self):
        def tamper(name):
            return self.files[name] + b"tampered"
        with patch.object(verify, "smoke") as smoke, self.assertRaises(ValueError):
            verify.download_assets(self.manifest, self.urls, self.root / "fresh", tamper)
        smoke.assert_not_called()

    def test_optimization_cannot_bypass_package_checks(self):
        folder, name = next(iter(release.PACKAGES.items()))
        destination = self.root / "fresh"
        verify.download_assets(self.manifest, self.urls, destination, self.files.__getitem__)
        package = destination / folder / name
        package.write_bytes(package.read_bytes() + b"tampered")
        with patch.dict(os.environ, {"PYTHONOPTIMIZE": "2"}):
            self.assertNotIn("PYTHONOPTIMIZE", release.verifier_environment())
            with self.assertRaises(release.subprocess.CalledProcessError):
                release.assets(destination)

    def test_download_failure_never_runs_smoke(self):
        def fail(name):
            raise OSError("public download unavailable")
        with patch.object(verify, "smoke") as smoke, self.assertRaises(OSError):
            verify.download_assets(self.manifest, self.urls, self.root / "fresh", fail)
        smoke.assert_not_called()


class Workflow(unittest.TestCase):
    def test_verification_has_no_writer_or_rebuild(self):
        workflow = (release.ROOT / ".github/workflows/ci.yml").read_text()
        verification = workflow.split("  verify-public-release:\n", 1)[1]
        self.assertNotIn("contents: write", verification)
        self.assertNotIn("GH_TOKEN", verification)
        self.assertNotIn("cargo ", verification)
        self.assertEqual(workflow.count("contents: write"), 1)
        self.assertIn("startsWith(github.ref, 'refs/heads/release/v')", workflow)
        self.assertEqual(workflow.count("!startsWith(github.ref, 'refs/heads/verify/v')"), 2)
        self.assertIn("os: [ubuntu-latest, macos-latest]", verification)

    def test_installer_default_matches_manifest(self):
        import tomllib
        version = tomllib.loads((release.ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
        default = re.search(r"^DEFAULT_VERSION=(\S+)$", (release.ROOT / "install.sh").read_text(), re.M)[1]
        self.assertEqual(default, "v" + version)


if __name__ == "__main__":
    unittest.main()
