#!/usr/bin/env python3
"""Opt-in official tunnel-client qualification; offline except loopback fixtures.

Accepts an already downloaded, checksum-pinned release ZIP. Never downloads,
installs, contacts an account/TV, or runs ADB/curl/cloudflared. Executes the verified
tunnel-client in a temporary HOME with only a dummy credential and loopback URL.
"""
import argparse
import datetime
import hashlib
import json
import os
import pathlib
import platform
import queue
import shlex
import signal
import subprocess
import tempfile
import threading
import time
import urllib.request
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

VERSION = "v0.0.17"
PINS = {
    "x86_64": ("linux-amd64", "bd6a57c6c0511644267cafadd13ffdb53ece0d753e0b88857f957e78a8fbc793"),
    "aarch64": ("linux-arm64", "f5060d0b63663268aab3ca310cb6110fa14867981db2b0657f679d8836a2daa0"),
}
TUNNEL = "tunnel_" + "a" * 32


def initialize(request_id):
    return {"jsonrpc": "2.0", "id": request_id, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {},
        "clientInfo": {"name": "isolated-tunnel-contract", "version": "1"}}}


def notification():
    return {"jsonrpc": "2.0", "method": "notifications/initialized"}


def tools_list(request_id):
    return {"jsonrpc": "2.0", "id": request_id, "method": "tools/list", "params": {}}


def verified_helper(archive, directory):
    if platform.system() != "Linux" or platform.machine() not in PINS:
        raise ValueError("This qualification requires Linux x86-64 or ARM64")
    target, expected = PINS[platform.machine()]
    digest = hashlib.sha256()
    with archive.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    if digest.hexdigest() != expected:
        raise ValueError("Official archive checksum mismatch; refusing execution")
    members = {"tunnel-client", "cloudflared", "cloudflared-manifest.json", "LICENSE", "NOTICE",
               f"tunnel-client-{VERSION}-{target}-licenses.txt",
               f"tunnel-client-{VERSION}-{target}.spdx.json"}
    with zipfile.ZipFile(archive) as zipped:
        if len(zipped.namelist()) != len(members) or set(zipped.namelist()) != members:
            raise ValueError("Unexpected official archive members")
        binary = directory / "tunnel-client"
        binary.write_bytes(zipped.read("tunnel-client"))
    binary.chmod(0o700)
    return binary


def exercise(directory, helper, controller, config, name, sequence, guard=False):
    case = directory / name
    case.mkdir()
    outgoing, incoming = queue.Queue(), queue.Queue()
    unexpected = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def answer(self, status, value=None):
            body = b"" if value is None else json.dumps(value).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass

        def do_GET(self):
            path = self.path.split("?", 1)[0]
            if path == f"/v1/tunnels/{TUNNEL}/poll":
                try:
                    self.answer(200, {"commands": [outgoing.get(timeout=0.2)]})
                except queue.Empty:
                    self.answer(204)
            elif path == f"/v1/tunnels/{TUNNEL}":
                self.answer(200, {"id": TUNNEL, "name": "isolated-fixture", "object": "tunnel"})
            else:
                unexpected.append(path)
                self.answer(404, {})

        def do_POST(self):
            if self.path != f"/v1/tunnels/{TUNNEL}/response":
                unexpected.append(self.path)
                self.answer(404, {})
                return
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length <= 65536:
                unexpected.append("invalid_response_length")
                self.answer(400, {})
                return
            incoming.put(json.loads(self.rfile.read(length)))
            self.answer(200, {})

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    health_file = case / "health.url"
    env = {"PATH": "/usr/bin:/bin", "HOME": str(case),
           "CONTROL_PLANE_API_KEY": "fixture_only_not_a_real_credential"}
    command = shlex.join([str(controller), "mcp", str(config)])
    args = [str(helper), "run", "--control-plane.base-url", f"http://127.0.0.1:{server.server_port}",
            "--control-plane.tunnel-id", TUNNEL, "--mcp.command", f"command={command},channel=main",
            "--health.listen-addr", "127.0.0.1:0", "--health.url-file", str(health_file),
            "--control-plane.poll-timeout", "1s", "--log.format", "json"]
    if guard:
        args.append("--mcp.stdio-send-initialized-notification")
    process = None
    events = []
    # Explicitly disable ambient proxy handling even for loopback health reads.
    http = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with (case / "runtime.log").open("w") as log:
            process = subprocess.Popen(args, env=env, stdin=subprocess.DEVNULL,
                                       stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            deadline = time.monotonic() + 8
            while not health_file.exists():
                if process.poll() is not None:
                    raise AssertionError(f"{name}: helper exited before health became available")
                if time.monotonic() >= deadline:
                    raise AssertionError(f"{name}: helper startup timed out")
                time.sleep(0.02)
            for index, request in enumerate(sequence):
                assert request["method"] in {"initialize", "notifications/initialized", "tools/list", "server/discover"}
                request_id = f"fixture-{index}"
                outgoing.put({"request_id": request_id, "shard_token": "fixture_dummy_shard",
                              "command_type": "jsonrpc", "channel": "main",
                              "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                              "response_timeout": "4s", "jsonrpc": request})
                response = incoming.get(timeout=6)
                assert response["request_id"] == request_id, response
                payload = response.get("resp_json", {})
                events.append({"method": request["method"], "code": payload.get("error", {}).get("code"),
                               "http_code": response.get("resp_code")})
            base = health_file.read_text().strip()
            if not base.startswith("http://127.0.0.1:"):
                raise AssertionError("Helper returned a non-loopback health URL")
            with http.open(base + "/health/mcp", timeout=2) as response:
                health = json.load(response)
            assert not unexpected, unexpected
    finally:
        if process is not None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=2)
        server.shutdown()
        server.server_close()
        server_thread.join(timeout=2)
    return events, health


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("controller", type=pathlib.Path)
    parser.add_argument("archive", type=pathlib.Path, help="Already downloaded official release ZIP")
    parser.add_argument("--config-template", type=pathlib.Path,
                        default=pathlib.Path(__file__).resolve().parent.parent / "examples/config.json")
    args = parser.parse_args()
    controller, archive = args.controller.resolve(strict=True), args.archive.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="tv-tunnel-contract-") as temporary:
        directory = pathlib.Path(temporary)
        helper = verified_helper(archive, directory)
        config_data = json.loads(args.config_template.read_text())
        config_data["executables"] = {"adb": str(directory / "never-run-adb"),
                                      "curl": str(directory / "never-run-curl")}
        config = directory / "fixture.json"
        config.write_text(json.dumps(config_data))
        sequence = []
        for cycle in range(3):
            sequence += [initialize(10 * cycle + 1), tools_list(10 * cycle + 2),
                         notification(), tools_list(10 * cycle + 3)]
        events, health = exercise(directory, helper, controller, config, "three_cycles", sequence)
        for index, event in enumerate(events):
            assert event["code"] == (-32002 if index % 4 == 1 else None), events
        assert health["status"] == "ok" and health["state"] == "discovered", health
        assert health["details"]["initialize_epoch"] == 3, health
        assert health["details"]["tools_list"]["tool_names"] == ["tv_capabilities", "tv_control"], health
        print("PASS: three same-child handshakes; tools gated until each fresh notification")

        malformed = initialize(3)
        malformed["params"]["clientInfo"]["version"] = None
        sequence = [initialize(1), notification(), tools_list(2), malformed,
                    tools_list(4), initialize(5), notification(), tools_list(6)]
        events, health = exercise(directory, helper, controller, config, "malformed_recovery", sequence)
        assert [event["code"] for event in events] == [None, None, None, -32602, -32002, None, None, None], events
        assert health["status"] == "ok" and health["details"]["initialize_epoch"] == 3, health
        print("PASS: malformed repeat rejected; later valid handshake restores discovery")

        sequence = [initialize(1), tools_list(2), initialize(3), tools_list(4), initialize(5), tools_list(6)]
        events, health = exercise(directory, helper, controller, config, "guard_enabled", sequence, guard=True)
        assert all(event["code"] is None for event in events), events
        assert health["status"] == "ok" and health["details"]["initialize_epoch"] == 3, health
        print("PASS: official initialized-notification compatibility guard across three cycles")

        modern = {"jsonrpc": "2.0", "id": "modern", "method": "server/discover", "params": {
            "_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                      "io.modelcontextprotocol/clientCapabilities": {}}}}
        sequence = [initialize(1), notification(), tools_list(2), modern,
                    initialize(3), notification(), tools_list(4)]
        events, health = exercise(directory, helper, controller, config, "modern_fallback", sequence)
        assert [event["code"] for event in events] == [None, None, None, -32601, None, None, None], events
        assert health["status"] == "ok" and health["state"] == "discovered", health
        assert health["details"]["initialize_epoch"] == 2, health
        assert health["details"]["server_discover_epoch"] == 1, health
        print("PASS: unsupported modern discovery followed by successful legacy reinitialization")

        sequence = [modern, initialize(1), notification(), tools_list(2),
                    initialize(3), notification(), tools_list(4)]
        events, health = exercise(directory, helper, controller, config, "modern_first", sequence)
        assert [event["code"] for event in events] == [-32600, None, None, None, None, None, None], events
        assert health["status"] == "ok" and health["state"] == "discovered", health
        assert health["details"]["initialize_epoch"] == 2, health
        assert health["details"]["server_discover_epoch"] == 1, health
        print("PASS: modern-first discovery falls back to two successful legacy handshakes")
    print("Official-client loopback qualification passed; no TV tools invoked")


if __name__ == "__main__":
    main()
