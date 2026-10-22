#!/usr/bin/env python3
"""Integration tests: real stdio binary + real curl against a loopback HTTP fixture.
Never contacts a TV, discovers devices, or uses an existing credential.
"""
import json
import os
import pathlib
import queue
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BINARY = "target/debug/tv-control"
ROOT = pathlib.Path(__file__).resolve().parent.parent

class Fixture(BaseHTTPRequestHandler):
    paused = False
    position = 200
    post_count = 0
    slow = False
    def log_message(self, *_):
        pass
    def do_GET(self):
        if self.slow:
            time.sleep(0.25)
        if self.path == "/System/Info/Public":
            value = {"Version": "12.0.0"}
        elif self.path.startswith("/Sessions?"):
            Fixture.position += 10
            value = [{"Id":"session", "DeviceId":"example_device", "UserId":"example_user",
                      "SupportsRemoteControl":True,"NowPlayingItem":{"Id":"episode"},
                      "PlayState":{"IsPaused":Fixture.paused,"PositionTicks":Fixture.position}}]
        elif self.path.startswith("/UserItems/Resume?"):
            value = {"Items":[{"Id":"episode","SeriesId":"example_series","UserData":{"PlaybackPositionTicks":100}}]}
        else:
            self.send_error(404)
            return
        data = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass
    def do_POST(self):
        Fixture.post_count += 1
        if self.path.endswith("/Pause"):
            Fixture.paused = True
        elif self.path.endswith("/Unpause"):
            Fixture.paused = False
        elif self.path.startswith("/Sessions/session/Playing?"):
            Fixture.paused = False
        else:
            self.send_error(404)
            return
        self.send_response(204)
        self.send_header("Content-Length", "0")
        self.end_headers()

class MCP(unittest.TestCase):
    def setUp(self):
        Fixture.paused = False
        Fixture.post_count = 0
        Fixture.slow = False
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.tmp = tempfile.TemporaryDirectory()
        cfg = json.loads((ROOT / "examples/config.json").read_text())
        cfg["servers"]["library"]["base_url"] = f"http://127.0.0.1:{self.server.server_port}"
        cfg["servers"]["library"]["allow_insecure_http"] = True
        cfg["preferences"]["verification_poll_ms"] = 5
        path = pathlib.Path(self.tmp.name) / "fixture.json"
        path.write_text(json.dumps(cfg))
        env = os.environ.copy()
        env["TV_LIBRARY_TOKEN"] = "fixture_only_not_a_credential"
        self.proc = subprocess.Popen([BINARY, "mcp", str(path)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
        self.responses = queue.Queue()
        def read():
            for line in self.proc.stdout:
                self.responses.put(json.loads(line))
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()
        self.send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"contract-test","version":"1"}}})
        self.assertEqual(self.receive()["result"]["protocolVersion"], "2025-11-25")
        self.send({"jsonrpc":"2.0","method":"notifications/initialized"})
    def tearDown(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=3)
        stderr = self.proc.stderr.read()
        self.reader.join(timeout=1)
        self.proc.stdout.close()
        self.proc.stderr.close()
        self.assertNotIn("fixture_only_not_a_credential", stderr)
        self.server.shutdown()
        self.server.server_close()
        self.tmp.cleanup()
    def send(self, value):
        self.proc.stdin.write(json.dumps(value) + "\n")
        self.proc.stdin.flush()
    def receive(self):
        return self.responses.get(timeout=3)
    def call(self, rpc_id, action, key="operation", ttl=1500):
        self.send({"jsonrpc":"2.0","id":rpc_id,"method":"tools/call","params":{"name":"tv_control","arguments":{
            "request_id":key,"device":"media_screen","action":action,"expires_at_unix_ms":int(time.time()*1000)+ttl}}})
    def test_tools_schema_and_unknown_arguments(self):
        self.send({"jsonrpc":"2.0","id":2,"method":"tools/list"})
        tools = self.receive()["result"]["tools"]
        self.assertEqual({t["name"] for t in tools}, {"tv_control", "tv_capabilities"})
        self.send({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tv_control","arguments":{"shell":"anything"}}})
        self.assertEqual(self.receive()["error"]["code"], -32602)
    def test_pause_postcondition_and_dedup(self):
        self.call(2, {"type":"pause"})
        first = self.receive()["result"]["structuredContent"]
        self.assertEqual(first["status"], "verified")
        self.call(3, {"type":"pause"})
        second = self.receive()["result"]["structuredContent"]
        self.assertTrue(second["cached"])
        self.assertEqual(Fixture.post_count, 1)
    def test_resume_media_verifies_progress(self):
        self.call(2, {"type":"resume_media","media":"series"})
        value = self.receive()["result"]["structuredContent"]
        self.assertEqual(value["status"], "verified")
        self.assertEqual(value["observed"]["media_id"], "episode")
    def test_expired_request_never_dispatches(self):
        self.call(2, {"type":"pause"}, ttl=-1)
        self.assertEqual(self.receive()["result"]["structuredContent"]["status"], "expired")
        self.assertEqual(Fixture.post_count, 0)
    def test_cancellation_notification_interrupts_inflight_read(self):
        Fixture.slow = True
        self.call(2, {"type":"pause"})
        time.sleep(0.03)
        self.send({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"test cancellation"}})
        value = self.receive()["result"]["structuredContent"]
        self.assertEqual(value["status"], "cancelled")
        self.assertEqual(Fixture.post_count, 0)
    def test_per_device_serialization(self):
        Fixture.slow = True
        self.call(2, {"type":"observe"}, key="first")
        time.sleep(0.03)
        self.call(3, {"type":"observe"}, key="second")
        result = self.receive()
        self.assertEqual(result["id"], 3)
        self.assertEqual(result["result"]["structuredContent"]["status"], "busy")
        self.assertEqual(self.receive()["id"], 2)

if __name__ == "__main__":
    if len(sys.argv) > 1 and not sys.argv[1].startswith("-"):
        BINARY = str(pathlib.Path(sys.argv.pop(1)).resolve())
    unittest.main()
