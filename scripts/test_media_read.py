#!/usr/bin/env python3
"""Offline CLI integration for media reads. No network, TV or account access."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1]).resolve())
root = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory() as directory:
    tmp = Path(directory)
    curl = tmp / "curl"
    log = tmp / "requests.jsonl"
    version_path = tmp / "version"
    version_path.write_text("12.0.0")
    curl.write_text("#!" + sys.executable + "\n" + "LOG = " + repr(str(log)) + "\nVERSION = " + repr(str(version_path)) + "\n" + '''import json, os, sys
raw = sys.stdin.read()
with open(LOG, "a") as log:
    # Log just the endpoint, never the authorization configuration.
    log.write(json.dumps(raw.splitlines()[0]) + "\\n")
if "/System/Info/Public" in raw:
    result = {"Version": open(VERSION).read()}
elif "/Items?" in raw:
    result = {"Items": [{"Id":"exact", "Name":"Example", "Type":"Movie", "Path":"private-path"}]}
else:
    raise SystemExit(2)
sys.stdout.write(json.dumps(result) + "\\n200")
''')
    curl.chmod(0o700)
    config = json.loads((root / "examples/config.json").read_text())
    config["executables"]["curl"] = str(curl)
    path = tmp / "config.json"
    path.write_text(json.dumps(config))
    env = dict(os.environ, TV_LIBRARY_TOKEN="test-fixture-only", MEDIA_FIXTURE_LOG=str(log))

    def run(read, expected=0, extra=None):
        request = {"device":"media_screen", "expires_at_unix_ms":int(time.time()*1000)+8000, "read":read}
        result = subprocess.run([binary, "media-read", str(path), "-"], input=json.dumps(request), text=True, capture_output=True, env=dict(env, **(extra or {})), timeout=12)
        assert result.returncode == expected, (result.returncode, result.stdout, result.stderr)
        assert "private-path" not in result.stdout and "test-fixture-only" not in result.stdout
        return json.loads(result.stdout)

    result = run({"type":"search", "query":"A&B", "limit":1})
    assert result["status"] == "observed" and result["items"][0]["id"] == "exact"
    assert "searchTerm=A%26B" in log.read_text()
    before = len(log.read_text().splitlines())
    result = run({"type":"item", "item_id":"wanted"}, 2)
    assert result["code"] == "jellyfin_item_identity_mismatch"
    version_path.write_text("10.11.0")
    result = run({"type":"search", "query":"x"}, 2)
    assert result["code"] == "jellyfin_server_requires_security_review"
    assert len(log.read_text().splitlines()) == before + 3
    result = run({"type":"search", "query":"x", "limit":0}, 2)
    assert result["code"] == "invalid_media_read"
    assert len(log.read_text().splitlines()) == before + 3
    assert not (tmp / ".tv-control-operations").exists()
print("media-read CLI offline contracts passed")
