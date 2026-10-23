#!/usr/bin/env python3
"""Read-only verification with authenticated metadata and anonymous public assets.

`plan RELEASE_ID SOURCE_SHA` prints an explicit verification branch. `verify DIR`
runs only on that branch and performs public download plus native package smoke.
Neither mode changes a release, tag, asset, or distribution branch.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.error
import urllib.parse
import urllib.request

import release

ROOT = release.ROOT
ASSET_NAMES = {"install.sh", "install.sh.sha256"}
ASSET_NAMES.update(name for package in release.PACKAGES.values() for name in (package, package + ".sha256"))


def manifest_digest(manifest):
    return hashlib.sha256(json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def candidate(api, release_id, sha, tag):
    """Read fixed release metadata and original publication provenance only."""
    release.require(isinstance(release_id, int) and release_id > 0, "invalid release ID")
    release.require(re.fullmatch(r"[0-9a-f]{40}", sha), "full source SHA required")
    release.require(re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag), "invalid version tag")
    base = f"/repos/{release.REPOSITORY}"
    item = api.request("GET", base + f"/releases/{release_id}")
    release.require(item.get("id") == release_id and item.get("tag_name") == tag
                    and item.get("draft") is False and item.get("prerelease") is True,
                    "selected public prerelease does not match")
    ref = api.request("GET", base + "/git/ref/tags/" + tag)
    release.require(ref.get("object", {}).get("type") == "commit" and ref["object"]["sha"] == sha, "source tag mismatch")
    body = item.get("body", "")
    release.require(re.findall(r"^Source commit: ([0-9a-f]{40})$", body, re.M) == [sha], "release source provenance mismatch")
    entries = re.findall(r"^([0-9a-f]{64})  (\S+)$", body, re.M)
    hashes = {name: digest for digest, name in entries}
    release.require(len(entries) == 8 and set(hashes) == ASSET_NAMES, "release checksum manifest mismatch")
    actual = item.get("assets", [])
    release.require(len(actual) == 8 and {asset.get("name") for asset in actual} == ASSET_NAMES, "public asset set mismatch")
    urls = {}
    for asset in actual:
        name = asset["name"]
        expected_url = f"https://github.com/{release.REPOSITORY}/releases/download/{tag}/{name}"
        release.require(asset.get("state") == "uploaded" and 0 < asset.get("size", 0) <= 20_000_000
                        and asset.get("digest") == "sha256:" + hashes[name]
                        and asset.get("browser_download_url") == expected_url, "public asset metadata mismatch")
        urls[name] = expected_url
    for label, branch in (("Main CI", "main"), ("Build and package CI", f"release/{tag}-{sha}")):
        pattern = "^" + re.escape(f"{label}: https://github.com/{release.REPOSITORY}/actions/runs/") + r"([1-9][0-9]*)$"
        ids = re.findall(pattern, body, re.M)
        release.require(len(ids) == 1, "missing CI provenance")
        run = api.request("GET", base + "/actions/runs/" + ids[0])
        release.require(run.get("head_sha") == sha and run.get("head_branch") == branch
                        and run.get("event") == "push" and run.get("path") == release.WORKFLOW
                        and run.get("status") == "completed" and run.get("conclusion") == "success"
                        and run.get("repository", {}).get("full_name") == release.REPOSITORY,
                        "original CI did not succeed for selected source")
    return {"release_id": release_id, "tag": tag, "source_sha": sha, "asset_sha256": hashes}, urls


def context(env, event, head, version):
    branch = env.get("GITHUB_REF", "").removeprefix("refs/heads/")
    pattern = re.escape(f"verify/v{version}-") + r"([0-9a-f]{40})-r([1-9][0-9]*)-([0-9a-f]{64})-c" + re.escape(head)
    match = re.fullmatch(pattern, branch)
    release.require(match is not None, "verification branch must bind source SHA, release ID, manifest and controller SHA")
    result = release.event_context(env, event, head, branch)
    return {**result, "controller_sha": head, "source_sha": match[1], "tag": "v" + version,
            "release_id": int(match[2]), "manifest_sha256": match[3]}


def source_checkout(root, selected):
    root = root.resolve()
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    release.require(head == selected["source_sha"], "source checkout differs from published source SHA")
    version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    default = re.search(r"^DEFAULT_VERSION=(\S+)$", (root / "install.sh").read_text(), re.M)
    release.require("v" + version == selected["tag"] and default is not None and default[1] == selected["tag"],
                    "source manifest or installer default mismatch")
    return root


class MetadataGitHub:
    """Only GET fixed-repository metadata. Never pass this token to downloads."""
    def __init__(self, token=None):
        self.token = token
        self.opener = urllib.request.build_opener(release.NoRedirect)

    def request(self, method, path):
        release.require(method == "GET", "metadata client is read-only")
        release.require(path.startswith(f"/repos/{release.REPOSITORY}/"), "unexpected metadata repository")
        headers = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28",
                   "User-Agent": "tv-control-public-verification"}
        if self.token:
            headers["Authorization"] = "Bearer " + self.token
        request = urllib.request.Request("https://api.github.com" + path, headers=headers, method="GET")
        try:
            with self.opener.open(request, timeout=60) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            # Allowlisted bounded response diagnostics only; never response bodies,
            # authorization headers, tokens, signed URLs, or automatic 403 retries.
            diagnostic = {"status": error.code}
            for name in ("X-RateLimit-Limit", "X-RateLimit-Remaining", "X-RateLimit-Reset", "Retry-After"):
                value = error.headers.get(name, "")
                if re.fullmatch(r"[0-9]{1,12}", value):
                    diagnostic[name.lower()] = value
            request_id = error.headers.get("X-GitHub-Request-Id", "")
            if re.fullmatch(r"[A-Fa-f0-9:]{1,80}", request_id):
                diagnostic["request_id"] = request_id
            error.close()
            raise RuntimeError("GitHub metadata GET failed: " + json.dumps(diagnostic, sort_keys=True)) from None


class PublicRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        parsed = urllib.parse.urlparse(newurl)
        release.require(parsed.scheme == "https" and parsed.hostname in {
            "github.com", "release-assets.githubusercontent.com", "objects.githubusercontent.com"
        }, "unexpected public download redirect")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def download(url):
    release.require(url.startswith(f"https://github.com/{release.REPOSITORY}/releases/download/"), "unexpected public URL")
    # No GitHub token, Authorization header, cookies, or credential store.
    request = urllib.request.Request(url, headers={"User-Agent": "tv-control-public-verification"})
    with urllib.request.build_opener(PublicRedirect).open(request, timeout=90) as response:
        data = response.read(20_000_001)
    release.require(len(data) <= 20_000_000, "public asset too large")
    return data


def download_assets(manifest, urls, destination, downloader=download, source_root=None):
    release.require(not destination.exists(), "verification destination must be fresh")
    files = {}
    for name in sorted(ASSET_NAMES):
        data = downloader(urls[name])
        release.require(hashlib.sha256(data).hexdigest() == manifest["asset_sha256"][name], "public download digest mismatch")
        files[name] = data
    for artifact, package in release.PACKAGES.items():
        folder = destination / artifact
        folder.mkdir(parents=True)
        for name in (package, package + ".sha256", "install.sh", "install.sh.sha256"):
            (folder / name).write_bytes(files[name])
    release.assets(destination, source_root=source_root)
    return files


def native_package():
    target = (sys.platform, platform.machine().lower())
    choices = {("linux", "x86_64"): "tv-control-x86_64-linux-musl",
               ("darwin", "arm64"): "tv-control-macos-ARM64"}
    release.require(target in choices, "compatible native smoke runner required")
    return choices[target]


def smoke(destination, manifest, source_root=None):
    source_root = source_root or ROOT
    artifact = native_package()
    folder = destination / artifact
    archive = folder / release.PACKAGES[artifact]
    with tempfile.TemporaryDirectory(prefix="tv-public-smoke-") as temporary:
        root = Path(temporary).resolve()
        home = root / "home"
        home.mkdir()
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(home), "TMPDIR": str(root), "LC_ALL": "C"}
        subprocess.run([sys.executable, str(source_root / "scripts/check_package.py"), str(archive), "--smoke"],
                       check=True, timeout=120, env=env)
        # Exercise the public installer/download path with the explicit version,
        # then compare the installed executable to the already verified archive.
        prefix = root / "public installed app"
        installer = folder / "install.sh"
        subprocess.run(["sh", str(installer), "--non-interactive", "--version", manifest["tag"],
                        "--prefix", str(prefix), "--json"], check=True, timeout=180, env=env)
        with tarfile.open(archive, "r:gz") as bundle:
            with bundle.extractfile("tv-control") as source:
                expected_binary = source.read()
        installed_binary = prefix / "versions" / manifest["tag"] / "tv-control"
        release.require(installed_binary.read_bytes() == expected_binary, "public installer binary differs from verified asset")
        output = subprocess.check_output([str(prefix / "tv-control"), "--version"], text=True, timeout=10, env=env)
        release.require(output.strip() == "tv-control " + manifest["tag"][1:], "installed version mismatch")
        fixture = prefix / "versions" / manifest["tag"] / "examples/discovery.json"
        output = subprocess.check_output([str(prefix / "tv-control"), "discover", "--fixture", str(fixture),
                                          "--interface", "lan-fixture", "--json"], text=True, timeout=10, env=env)
        discovery = json.loads(output)
        release.require(discovery.get("simulation") is True and len(discovery.get("candidates", [])) == 3, "installed fixture discovery failed")
        subprocess.run(["sh", str(installer), "--non-interactive", "--uninstall", "--prefix", str(prefix), "--json"],
                       check=True, timeout=30, env=env)
        release.require(not prefix.exists(), "public installation was not removed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    plan = commands.add_parser("plan")
    plan.add_argument("release_id", type=int)
    plan.add_argument("source_sha")
    verify = commands.add_parser("verify")
    verify.add_argument("destination", type=Path)
    verify.add_argument("--source-root", type=Path, required=True)
    commands.add_parser("select")
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    release.require(re.search(r"^DEFAULT_VERSION=(\S+)$", (ROOT / "install.sh").read_text(), re.M).group(1) == "v" + version,
                    "installer default must match the release version")
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    api = MetadataGitHub(os.environ.get("GH_METADATA_TOKEN"))
    if args.command == "plan":
        manifest, _ = candidate(api, args.release_id, args.source_sha, "v" + version)
        digest = manifest_digest(manifest)
        print(json.dumps({"branch": f"verify/v{version}-{args.source_sha}-r{args.release_id}-{digest}-c{head}",
                          "manifest_sha256": digest, "manifest": manifest}, indent=2))
        return
    selected = context(os.environ, json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text()), head, version)
    if args.command == "select":
        # Values are fixed-format validated SHA/IDs, so cannot inject output lines.
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            output.write("source_sha=" + selected["source_sha"] + "\n")
        print(json.dumps(selected, indent=2))
        return
    release.require(bool(api.token), "ephemeral read-only metadata token required")
    source_root = source_checkout(args.source_root, selected)
    manifest, urls = candidate(api, selected["release_id"], selected["source_sha"], selected["tag"])
    release.require(manifest_digest(manifest) == selected["manifest_sha256"], "selected release manifest changed")
    download_assets(manifest, urls, args.destination, source_root=source_root)
    smoke(args.destination, manifest, source_root=source_root)
    # Re-read after execution so a changed release is not silently certified.
    after, _ = candidate(api, selected["release_id"], selected["source_sha"], selected["tag"])
    release.require(after == manifest, "public release changed during verification")
    print(json.dumps({"verified": True, "manifest": manifest, "manifest_sha256": selected["manifest_sha256"],
                      "native_smoke": native_package(), "controller_sha": selected["controller_sha"],
                      "source_sha": selected["source_sha"], "remote_writes": False}, indent=2))


if __name__ == "__main__":
    main()
