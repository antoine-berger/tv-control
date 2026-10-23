#!/usr/bin/env python3
"""Publish an explicitly selected, same-run CI candidate as a non-latest prerelease.

Without --publish this performs local validation only. Never executes a downloaded
binary or installer, overwrites an asset, moves a tag, or promotes a release.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tomllib
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
REPOSITORY = "antoine-berger/tv-control"
WORKFLOW = ".github/workflows/ci.yml"
PACKAGES = {
    "tv-control-macos-ARM64": "tv-control-aarch64-apple-darwin.tar.gz",
    "tv-control-aarch64-linux-musl": "tv-control-aarch64-linux-musl.tar.gz",
    "tv-control-x86_64-linux-musl": "tv-control-x86_64-linux-musl.tar.gz",
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def event_context(env, event, head, branch):
    """Common immutable push/checkout/run identity checks, without publication."""
    sha = env.get("GITHUB_SHA", "")
    require(re.fullmatch(r"[0-9a-f]{40}", sha), "full commit SHA required")
    ref = "refs/heads/" + branch
    require(env.get("GITHUB_ACTIONS") == "true", "GitHub Actions required")
    require(env.get("GITHUB_REPOSITORY") == REPOSITORY, "unexpected repository")
    require(env.get("GITHUB_EVENT_NAME") == "push", "push event required")
    require(env.get("GITHUB_REF_TYPE") == "branch", "branch event required")
    require(env.get("GITHUB_REF") == ref, "branch must bind version, full SHA and selected release")
    require(env.get("GITHUB_WORKFLOW_REF") == f"{REPOSITORY}/{WORKFLOW}@{ref}", "unexpected workflow ref")
    require(head == sha, "checkout differs from selected commit")
    require(event.get("repository", {}).get("full_name") == REPOSITORY, "unexpected event repository")
    require(event.get("ref") == ref and event.get("after") == sha, "event ref or SHA mismatch")
    require(event.get("deleted") is False and event.get("forced") is False, "deleted or forced push forbidden")
    for key in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"):
        require(re.fullmatch(r"[1-9][0-9]*", env.get(key, "")), f"invalid {key}")
    return {"sha": sha, "branch": branch, "run_id": int(env["GITHUB_RUN_ID"]),
            "run_attempt": int(env["GITHUB_RUN_ATTEMPT"])}


def context(env, event, head, version):
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version), "stable manifest version required")
    branch = f"release/v{version}-{env.get('GITHUB_SHA', '')}"
    result = event_context(env, event, head, branch)
    require(env.get("CHECKS_RESULT") == "success" and env.get("STATIC_LINUX_RESULT") == "success", "all required CI jobs must succeed")
    return {**result, "tag": "v" + version}


def verifier_environment():
    # check_package uses assertions; never let inherited optimization disable them.
    return {key: value for key, value in os.environ.items() if key not in {"PYTHONOPTIMIZE", "GH_TOKEN", "GITHUB_TOKEN", "GH_METADATA_TOKEN",
                                                       "ACTIONS_RUNTIME_TOKEN", "ACTIONS_ID_TOKEN_REQUEST_TOKEN"}}


def assets(directory, source_root=None):
    """Require exact artifacts and compare bundled source to its own checkout."""
    source_root = source_root or ROOT
    require(directory.is_dir() and not directory.is_symlink(), "artifact directory missing or linked")
    require({p.name for p in directory.iterdir()} == set(PACKAGES), "unexpected or missing artifact directories")
    result = {}
    for artifact, name in PACKAGES.items():
        folder = directory / artifact
        expected = {name, name + ".sha256", "install.sh", "install.sh.sha256"}
        require(folder.is_dir() and not folder.is_symlink(), "artifact directory must be local")
        require({p.name for p in folder.iterdir()} == expected, "unexpected or missing artifact files")
        for path in folder.iterdir():
            require(path.is_file() and not path.is_symlink() and path.stat().st_size <= 20_000_000,
                    "invalid artifact file")
        subprocess.run([sys.executable, str(source_root / "scripts/check_package.py"), str(folder / name)],
                       check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=verifier_environment())
        with tarfile.open(folder / name, "r:gz") as archive:
            for member in archive.getmembers():
                if member.name != "tv-control":
                    with archive.extractfile(member) as source:
                        require(source.read() == (source_root / member.name).read_bytes(), "archive source differs from selected commit")
        for path in sorted(folder.iterdir()):
            data = path.read_bytes()
            require(path.name not in result or result[path.name] == data, "duplicate installer assets differ")
            result[path.name] = data
    require(len(result) == 8, "exactly eight release assets required")
    require(result["install.sh"] == (source_root / "install.sh").read_bytes(), "installer source mismatch")
    return result


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("API redirects are forbidden")


class GitHub:
    def __init__(self, token=None, anonymous=False):
        require(bool(token) or anonymous, "GH_TOKEN required for publication")
        self.token = token
        self.opener = urllib.request.build_opener(NoRedirect)

    def request(self, method, path, payload=None, binary=False, optional=False):
        host = "https://uploads.github.com" if binary else "https://api.github.com"
        require(path.startswith(f"/repos/{REPOSITORY}/"), "unexpected API repository")
        data = payload if binary else (json.dumps(payload).encode() if payload is not None else None)
        require(bool(self.token) or method == "GET", "anonymous client is read-only")
        headers = {
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28", "User-Agent": "tv-control-release",
            "Content-Type": "application/octet-stream" if binary else "application/json",
        }
        if self.token:
            headers["Authorization"] = "Bearer " + self.token
        req = urllib.request.Request(host + path, data=data, method=method, headers=headers)
        try:
            with self.opener.open(req, timeout=60) as response:
                return json.load(response)
        except urllib.error.HTTPError as exc:
            # Never retry writes: a lost response can conceal a successful mutation.
            if optional and exc.code == 404:
                return None
            raise RuntimeError(f"GitHub {method} failed with HTTP {exc.code}; inspect state before retrying") from None


def publish(api, selection, files):
    base = f"/repos/{REPOSITORY}"
    sha, tag, run_id = selection["sha"], selection["tag"], selection["run_id"]
    main = api.request("GET", base + "/git/ref/heads/main")
    require(main.get("object", {}).get("type") == "commit" and main["object"]["sha"] == sha,
            "selected commit must still be main HEAD")
    current = api.request("GET", base + f"/actions/runs/{run_id}")
    require(current.get("head_sha") == sha and current.get("head_branch") == selection["branch"]
            and current.get("event") == "push" and current.get("path") == WORKFLOW
            and current.get("repository", {}).get("full_name") == REPOSITORY
            and current.get("run_attempt") == selection["run_attempt"], "current run provenance mismatch")
    query = urllib.parse.urlencode({"branch": "main", "event": "push", "head_sha": sha,
                                    "status": "success", "per_page": 100})
    runs = api.request("GET", base + "/actions/workflows/ci.yml/runs?" + query)
    passed = [run for run in runs.get("workflow_runs", []) if
              run.get("head_sha") == sha and run.get("head_branch") == "main"
              and run.get("event") == "push" and run.get("path") == WORKFLOW
              and run.get("status") == "completed" and run.get("conclusion") == "success"
              and run.get("repository", {}).get("full_name") == REPOSITORY]
    require(passed, "no successful main CI run for the exact selected commit")
    require(api.request("GET", base + "/git/ref/tags/" + tag, optional=True) is None, "tag already exists; refusing to move it")
    require(api.request("GET", base + "/releases/tags/" + tag, optional=True) is None, "release already exists; refusing to overwrite it")
    # Drafts might not resolve through the by-tag endpoint; also inspect all pages.
    page = 1
    while True:
        releases = api.request("GET", base + f"/releases?per_page=100&page={page}")
        require(all(release.get("tag_name") != tag for release in releases), "draft or published release already exists")
        if len(releases) < 100:
            break
        page += 1
    hashes = {name: hashlib.sha256(data).hexdigest() for name, data in sorted(files.items())}
    body = "\n".join([
        "Release candidate; not promoted to latest. Public redownload verification is still required.", "",
        "Source commit: " + sha,
        f"Main CI: https://github.com/{REPOSITORY}/actions/runs/{passed[0]['id']}",
        f"Build and package CI: https://github.com/{REPOSITORY}/actions/runs/{run_id}",
        "", "SHA-256 assets:", *[f"{digest}  {name}" for name, digest in hashes.items()],
    ])
    # The tag and draft may remain if a later operation fails. Never delete or repair
    # them automatically: a maintainer must inspect any partial publication.
    created_tag = api.request("POST", base + "/git/refs", {"ref": "refs/tags/" + tag, "sha": sha})
    require(created_tag.get("object", {}).get("sha") == sha, "created tag SHA mismatch")
    release = api.request("POST", base + "/releases", {
        "tag_name": tag, "target_commitish": sha, "name": tag, "body": body,
        "draft": True, "prerelease": True, "make_latest": "false",
    })
    release_id = release.get("id")
    require(isinstance(release_id, int) and release_id > 0 and release.get("draft") is True,
            "unexpected draft release response")
    for name, data in sorted(files.items()):
        uploaded = api.request("POST", base + f"/releases/{release_id}/assets?" + urllib.parse.urlencode({"name": name}),
                               data, binary=True)
        require(uploaded.get("name") == name and uploaded.get("size") == len(data)
                and uploaded.get("state") == "uploaded"
                and uploaded.get("digest") == "sha256:" + hashes[name], "uploaded asset verification failed")
    release = api.request("GET", base + f"/releases/{release_id}")
    require(release.get("draft") is True and release.get("prerelease") is True and release.get("tag_name") == tag,
            "draft metadata changed")
    actual = release.get("assets", [])
    require(len(actual) == len(files) and {a.get("name") for a in actual} == set(files), "draft asset set mismatch")
    for asset in actual:
        name = asset["name"]
        require(asset.get("state") == "uploaded" and asset.get("size") == len(files[name])
                and asset.get("digest") == "sha256:" + hashes[name], "draft asset digest mismatch")
    ref = api.request("GET", base + "/git/ref/tags/" + tag)
    require(ref.get("object", {}).get("type") == "commit" and ref["object"]["sha"] == sha, "tag moved during publication")
    published = api.request("PATCH", base + f"/releases/{release_id}", {
        "draft": False, "prerelease": True, "make_latest": "false",
    })
    require(published.get("draft") is False and published.get("prerelease") is True
            and published.get("tag_name") == tag, "prerelease publication not confirmed")
    return {"release_id": release_id, "tag": tag, "source_sha": sha,
            "url": f"https://github.com/{REPOSITORY}/releases/tag/{tag}",
            "prerelease": True, "promoted": False, "asset_sha256": hashes}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifacts", type=Path)
    parser.add_argument("--publish", action="store_true")
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    selection = context(os.environ, event, head, version)
    files = assets(args.artifacts)
    if args.publish:
        result = publish(GitHub(os.environ.get("GH_TOKEN")), selection, files)
    else:
        result = {**selection, "validated_assets": sorted(files), "published": False}
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
