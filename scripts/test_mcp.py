#!/usr/bin/env python3
"""Integration tests: real stdio binary + real curl against a loopback HTTP fixture.
Never contacts a TV, discovers devices, or uses an existing credential.
"""
import json
import fcntl
import os
import pathlib
import queue
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from urllib.parse import parse_qs, urlsplit
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BINARY = "target/debug/tv-control"
ROOT = pathlib.Path(__file__).resolve().parent.parent

class Fixture(BaseHTTPRequestHandler):
    paused = False
    position = 200
    post_count = 0
    slow = False
    position_mode = "progressing"
    ignore_seek = False
    def log_message(self, *_):
        pass
    def do_GET(self):
        if self.slow:
            time.sleep(0.25)
        if self.path == "/System/Info/Public":
            value = {"Version": "12.0.0"}
        elif self.path.startswith("/Sessions?"):
            if Fixture.position_mode == "progressing":
                Fixture.position += 10
            state = {"IsPaused":Fixture.paused}
            if Fixture.position_mode != "missing":
                state["PositionTicks"] = None if Fixture.position_mode == "null" else Fixture.position
            value = [{"Id":"session", "DeviceId":"example_device", "UserId":"example_user",
                      "SupportsRemoteControl":True,"NowPlayingItem":{"Id":"episode"},
                      "PlayState":state}]
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
            query = parse_qs(urlsplit(self.path).query)
            Fixture.position = 1_000_000_000 if Fixture.ignore_seek else int(query["startPositionTicks"][0])
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
        Fixture.position_mode = "progressing"
        Fixture.ignore_seek = False
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.tmp = tempfile.TemporaryDirectory()
        cfg = json.loads((ROOT / "examples/config.json").read_text())
        cfg["servers"]["library"]["base_url"] = f"http://127.0.0.1:{self.server.server_port}"
        cfg["servers"]["library"]["allow_insecure_http"] = True
        cfg["preferences"]["verification_poll_ms"] = 5
        path = pathlib.Path(self.tmp.name) / "fixture.json"
        path.write_text(json.dumps(cfg))
        self.config_path = path
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
    def receive(self, timeout=3):
        return self.responses.get(timeout=timeout)
    def call(self, rpc_id, action, key="operation", ttl=1500):
        self.send({"jsonrpc":"2.0","id":rpc_id,"method":"tools/call","params":{"name":"tv_control","arguments":{
            "request_id":key,"device":"media_screen","action":action,"expires_at_unix_ms":int(time.time()*1000)+ttl}}})
    def test_tools_schema_and_unknown_arguments(self):
        self.send({"jsonrpc":"2.0","id":2,"method":"tools/list"})
        tools = self.receive()["result"]["tools"]
        self.assertEqual({t["name"] for t in tools}, {"tv_control", "tv_capabilities", "tv_read", "tv_operation"})
        self.send({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tv_control","arguments":{"shell":"anything"}}})
        self.assertEqual(self.receive()["error"]["code"], -32602)
    def reinitialize(self, rpc_id):
        self.send({"jsonrpc":"2.0","id":rpc_id,"method":"initialize","params":{
            "protocolVersion":"2025-11-25","capabilities":{},
            "clientInfo":{"name":"next-tunnel-client","version":"1"}}})
        response = self.receive()
        self.assertEqual(response["id"], rpc_id)
        self.assertEqual(response["result"]["protocolVersion"], "2025-11-25")

    def test_reinitialize_requires_fresh_notification_each_cycle(self):
        # Official tunnel-client shares one child across client discovery sessions.
        for rpc_id in (10, 20, 30):
            self.reinitialize(rpc_id)
            self.send({"jsonrpc":"2.0","id":rpc_id+1,"method":"tools/list"})
            self.assertEqual(self.receive()["error"]["message"], "initialize_first")
            self.send({"jsonrpc":"2.0","method":"notifications/initialized"})
            self.send({"jsonrpc":"2.0","id":rpc_id+2,"method":"tools/list"})
            self.assertEqual(len(self.receive()["result"]["tools"]), 4)
        self.assertEqual(Fixture.post_count, 0)

    def test_malformed_reinitialize_preserves_validated_session(self):
        for params in ({}, {"protocolVersion":42,"capabilities":{},
                           "clientInfo":{"name":"invalid","version":"1"}}):
            self.send({"jsonrpc":"2.0","id":10,"method":"initialize","params":params})
            self.assertEqual(self.receive()["error"]["code"], -32602)
            self.send({"jsonrpc":"2.0","id":11,"method":"tools/list"})
            self.assertEqual(len(self.receive()["result"]["tools"]), 4)
        self.reinitialize(12)
        # Even an interrupted handshake can be replaced by another valid one.
        self.reinitialize(13)
        self.send({"jsonrpc":"2.0","method":"notifications/initialized"})
        self.send({"jsonrpc":"2.0","id":14,"method":"tools/list"})
        self.assertEqual(len(self.receive()["result"]["tools"]), 4)

    def test_reinitialize_preserves_action_deduplication(self):
        self.call(2, {"type":"pause"})
        self.assertEqual(self.receive()["result"]["structuredContent"]["status"], "verified")
        self.reinitialize(3)
        self.send({"jsonrpc":"2.0","method":"notifications/initialized"})
        self.call(4, {"type":"pause"})
        self.assertTrue(self.receive()["result"]["structuredContent"]["cached"])
        self.assertEqual(Fixture.post_count, 1)

    def test_reinitialize_preserves_inflight_cancellation(self):
        Fixture.slow = True
        self.call(2, {"type":"pause"})
        time.sleep(0.03)
        self.reinitialize(3)
        self.send({"jsonrpc":"2.0","method":"notifications/cancelled",
                   "params":{"requestId":2,"reason":"test cancellation after reinitialize"}})
        response = self.receive()
        self.assertEqual(response["id"], 2)
        self.assertEqual(response["result"]["structuredContent"]["status"], "cancelled")
        self.assertEqual(Fixture.post_count, 0)

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

    def test_resume_position_evidence_modes(self):
        for index, mode in enumerate(["missing", "null", "frozen"]):
            Fixture.position_mode = mode
            Fixture.paused = True
            self.call(index+2, {"type":"resume"}, key="evidence_"+mode, ttl=2500)
            value = self.receive(timeout=6)["result"]["structuredContent"]
            self.assertEqual(value["status"], "outcome_unknown", mode)
            self.assertFalse(value["cached"])

    def test_resume_media_rejects_ignored_seek(self):
        Fixture.ignore_seek = True
        self.call(2, {"type":"resume_media","media":"series"})
        value = self.receive()["result"]["structuredContent"]
        self.assertEqual(value["status"], "outcome_unknown")
        self.assertEqual(value["code"], "resume_position_not_confirmed")
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

    def test_minimum_output_config_handshake_tools_list_and_ping(self):
        cfg = json.loads(self.config_path.read_text())
        cfg["preferences"]["max_output_bytes"] = 4096
        cfg["preferences"]["max_frame_bytes"] = 1024
        path = pathlib.Path(self.tmp.name) / "minimum-output.json"
        path.write_text(json.dumps(cfg))
        proc = subprocess.Popen([BINARY, "mcp", str(path)], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        responses = queue.Queue()
        def consume():
            for line in proc.stdout:
                responses.put(line)
        reader = threading.Thread(target=consume, daemon=True)
        reader.start()
        def exchange(frame):
            proc.stdin.write(json.dumps(frame).encode()+b"\n")
            proc.stdin.flush()
            data = responses.get(timeout=3)
            self.assertLessEqual(len(data), 4096)
            return json.loads(data)
        try:
            result = exchange({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"minimum-test","version":"1"}}})
            self.assertEqual(result["result"]["protocolVersion"], "2025-11-25")
            proc.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
            proc.stdin.flush()
            result = exchange({"jsonrpc":"2.0","id":2,"method":"tools/list"})
            first=result["result"];names=[t["name"] for t in first["tools"]]
            self.assertIn("tv_control",names)  # legacy onboarding probes first page
            cursor=first.get("nextCursor");seen=set();rpc_id=3
            while cursor is not None:
                self.assertNotIn(cursor,seen);seen.add(cursor)
                result=exchange({"jsonrpc":"2.0","id":rpc_id,"method":"tools/list","params":{"cursor":cursor}});rpc_id+=1
                names.extend(t["name"] for t in result["result"]["tools"])
                cursor=result["result"].get("nextCursor")
            self.assertEqual(len(names),len(set(names)))
            self.assertEqual(set(names), {"tv_control", "tv_capabilities", "tv_read", "tv_operation"})
            self.assertEqual(exchange({"jsonrpc":"2.0","id":rpc_id,"method":"ping"})["result"], {})
            self.assertIsNone(proc.poll())
        finally:
            proc.stdin.close()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=2)
            reader.join(timeout=1)
            proc.stdout.close()
            proc.stderr.close()
            self.assertFalse(reader.is_alive())

    def test_unread_stdout_has_bounded_teardown(self):
        environment = os.environ.copy()
        environment["TV_LIBRARY_TOKEN"] = "fixture_only_not_a_credential"
        proc = subprocess.Popen([BINARY, "mcp", str(self.config_path)], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment)
        writer = None
        try:
            proc.stdin.write(json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"backpressure-test","version":"1"}}}).encode()+b"\n")
            proc.stdin.flush()
            self.assertIn(b'"result"', proc.stdout.readline())
            # Stop reading stdout. A blocking transport would deadlock while this producer fills stdin.
            frames = [{"jsonrpc":"2.0","method":"notifications/initialized"}]
            frames += [{"jsonrpc":"2.0","id":i+2,"method":"tools/list"} for i in range(2000)]
            frames += [{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}}]
            data = b"".join(json.dumps(frame).encode()+b"\n" for frame in frames)
            def produce():
                try:
                    proc.stdin.write(data)
                    proc.stdin.flush()
                except (BrokenPipeError, OSError):
                    pass
            writer = threading.Thread(target=produce, daemon=True)
            started = time.monotonic()
            writer.start()
            proc.wait(timeout=3)
            writer.join(timeout=1)
            self.assertFalse(writer.is_alive())
            self.assertLess(time.monotonic()-started, 3)
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=2)
            if writer:
                writer.join(timeout=1)
            for stream in [proc.stdin, proc.stdout, proc.stderr]:
                try:
                    stream.close()
                except BrokenPipeError:
                    pass

    def test_inherited_stdout_status_flags_are_restored(self):
        for nonblocking in (False, True):
            with self.subTest(nonblocking=nonblocking):
                self.check_stdout_flags_restored(nonblocking)

    def check_stdout_flags_restored(self, nonblocking):
        read_fd, write_fd = os.pipe()
        # Darwin adds kernel-only FWASWRITTEN (0x10000) on first write, even
        # from plain os.write; F_SETFL cannot clear it. Prime before recording
        # the complete baseline rather than masking any bits from the assertion.
        os.write(write_fd, b"x")
        self.assertEqual(os.read(read_fd, 1), b"x")
        if nonblocking:
            fcntl.fcntl(write_fd, fcntl.F_SETFL,
                        fcntl.fcntl(write_fd, fcntl.F_GETFL) | os.O_NONBLOCK)
        flags = fcntl.fcntl(write_fd, fcntl.F_GETFL)
        proc = subprocess.Popen([BINARY,"mcp",str(self.config_path)],stdin=subprocess.PIPE,
                                stdout=write_fd,stderr=subprocess.PIPE)
        reader = os.fdopen(read_fd,"rb")
        try:
            proc.stdin.write(json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"flags-test","version":"1"}}}).encode()+b"\n")
            proc.stdin.flush()
            self.assertIn(b'"result"',reader.readline())
            proc.stdin.close()
            proc.wait(timeout=3)
            self.assertEqual(fcntl.fcntl(write_fd,fcntl.F_GETFL),flags)
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=2)
            reader.close()
            os.close(write_fd)
            proc.stderr.close()
            if not proc.stdin.closed:
                proc.stdin.close()

if __name__ == "__main__":
    if len(sys.argv) > 1 and not sys.argv[1].startswith("-"):
        BINARY = str(pathlib.Path(sys.argv.pop(1)).resolve())
    unittest.main()
