#!/usr/bin/env python3
"""Advanced control contracts through the real MCP binary and local fake backends.

No TV, saved configuration, existing token or public server is used.
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
from urllib.parse import parse_qs, urlsplit

import test_generic_mcp as generic
from mcp_fixture_phase import VerificationBarrier

BINARY = str(generic.BINARY)
ROOT = pathlib.Path(__file__).resolve().parents[1]


class Volume(generic.GenericMcp):
    def test_schema_pagination_with_long_rpc_id_fits_minimum_frame(self):
        self.close_process()
        config = json.loads(self.config.read_text())
        config["preferences"]["max_output_bytes"] = 4096
        self.config.write_text(json.dumps(config))
        self.start()
        cursor = None
        tools = []
        while True:
            identity = "x" * 64
            self.write({"jsonrpc": "2.0", "id": identity, "method": "tools/list",
                        "params": {} if cursor is None else {"cursor": cursor}})
            line = self.process.stdout.readline()
            self.assertLess(len(line), 4096)
            reply = json.loads(line)
            self.assertEqual(reply["id"], identity)
            self.assertNotIn("error", reply)
            tools += reply["result"]["tools"]
            cursor = reply["result"].get("nextCursor")
            if cursor is None:
                break
        self.assertIn("tv_control", [tool["name"] for tool in tools])
        self.assertEqual(len(tools), 5)
        control = next(tool for tool in tools if tool["name"] == "tv_control")
        for semantic in ("native-scale", "global", "dispatch-only", "readback"):
            self.assertIn(semantic, control["description"])
        self.assertIn("remote_media: global, dispatch-only", control["description"])
        delta = control["inputSchema"]["properties"]["action"]["properties"]["delta"]
        self.assertIn("native", delta["description"])
        self.assertIn("index", delta["description"])

    def test_native_scale_targets_survive_restart_and_reject_conflict(self):
        path = self.root / "state.json"
        state = json.loads(path.read_text())
        state.update(volume=5, volume_max=17)
        path.write_text(json.dumps(state))
        action = {"type": "set_volume_percent", "percent": 50}
        result = self.control("half_scale", action)["structuredContent"]
        self.assertEqual(result["status"], "verified", result)
        self.assertEqual(result["state"]["volume_index"], 9)
        self.close_process()
        self.start()
        result = self.control("half_scale", action)["structuredContent"]
        self.assertTrue(result["cached"], result)
        self.assertEqual(json.loads(path.read_text())["volume_sets"], [9])
        result = self.control("half_scale", {"type": "adjust_volume", "delta": -1})["structuredContent"]
        self.assertEqual(result["status"], "invalid", result)
        result = self.control("one_lower", {"type": "adjust_volume", "delta": -1})["structuredContent"]
        self.assertEqual(result["status"], "verified", result)
        self.assertEqual(json.loads(path.read_text())["volume_sets"], [9, 8])

    def test_volume_bounds_fail_without_writes_and_transport_stays_usable(self):
        path = self.root / "state.json"
        state = json.loads(path.read_text())
        state.update(volume=0, volume_max=17)
        path.write_text(json.dumps(state))
        for key, action in [("underflow", {"type": "adjust_volume", "delta": -1}),
                            ("percentage", {"type": "set_volume_percent", "percent": 101})]:
            result = self.control(key, action)
            self.assertTrue(result["isError"], result)
            self.assertNotEqual(result["structuredContent"]["status"], "verified")
        self.assertNotIn("volume_sets", json.loads(path.read_text()))
        for percent, target in [(0, 0), (100, 17)]:
            result = self.control("endpoint_" + str(percent), {"type": "set_volume_percent", "percent": percent})
            self.assertEqual(result["structuredContent"]["status"], "verified", result)
            self.assertEqual(result["structuredContent"]["state"]["volume_index"], target)
        self.assertEqual(self.rpc("ping", {}), {})


class MediaFixture(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        state = self.server.state
        if self.path == "/System/Info/Public":
            value = {"Version": "12.0.0"}
        elif self.path.startswith("/Sessions?"):
            barrier = self.server.verification_barrier
            if barrier is not None:
                barrier.before_session_response()
            if state["playing"]:
                state["position"] += 100_000  # Natural progress, without any seek.
            value = [{"Id": "session", "DeviceId": "example_device", "UserId": "example_user",
                      "SupportsRemoteControl": True, "SupportsMediaControl": True,
                      "SupportedCommands": state["commands"], "NowPlayingItem": {"Id": "episode"},
                      "PlayState": {"IsPaused": not state["playing"], "CanSeek": True, "MediaSourceId": "source",
                                    "PositionTicks": state["position"], "AudioStreamIndex": state["audio"],
                                    "SubtitleStreamIndex": state["subtitle"]}}]
        elif self.path.startswith("/Items?"):
            value = {"Items": [{"Id": "episode", "Name": "Fixture", "Type": "Episode",
                                "MediaSources": [{"Id": "source", "RunTimeTicks": 100_000_000,
                                                  "MediaStreams": [{"Index": 1, "Type": "Audio"},
                                                                   {"Index": 4, "Type": "Subtitle"}]}]}]}
        else:
            self.send_error(404)
            return
        body = json.dumps(value).encode()
        try:
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_POST(self):
        state = self.server.state
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        state["posts"].append({"path": self.path, "body": body.decode()})
        if self.path == "/Sessions/session/Command":
            command = json.loads(body)
            if not state["ignore"]:
                field = {"SetAudioStreamIndex": "audio", "SetSubtitleStreamIndex": "subtitle"}[command["Name"]]
                state[field] = int(command["Arguments"]["Index"])
        elif self.path.startswith("/Sessions/session/Playing/Seek?"):
            if not state["ignore_seek"]:
                state["position"] = int(parse_qs(urlsplit(self.path).query)["seekPositionTicks"][0])
        else:
            self.send_error(404)
            return
        barrier = self.server.verification_barrier
        if barrier is not None:
            barrier.record_post()
        self.send_response(204)
        self.send_header("Content-Length", "0")
        self.end_headers()


class Media(unittest.TestCase):
    def setUp(self):
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), MediaFixture)
        self.server.verification_barrier = None
        self.server.state = {"commands": ["SetAudioStreamIndex", "SetSubtitleStreamIndex"],
                             "position": 50_000_000, "audio": 0, "subtitle": None,
                             "ignore": False, "ignore_seek": False, "playing": False, "posts": []}
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.tmp = tempfile.TemporaryDirectory()
        cfg = json.loads((ROOT / "examples/config.json").read_text())
        cfg["servers"]["library"]["base_url"] = f"http://127.0.0.1:{self.server.server_port}"
        cfg["servers"]["library"]["allow_insecure_http"] = True
        cfg["preferences"]["verification_poll_ms"] = 5
        self.path = pathlib.Path(self.tmp.name) / "fixture.json"
        self.path.write_text(json.dumps(cfg))
        self.serial = 0
        self.start()

    def start(self):
        self.proc = subprocess.Popen([BINARY, "mcp", str(self.path)], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                     env=dict(os.environ, TV_LIBRARY_TOKEN="fixture_only"))
        self.responses = queue.Queue()
        def read():
            for line in self.proc.stdout:
                self.responses.put(json.loads(line))
        self.reader = threading.Thread(target=read)
        self.reader.start()
        self.rpc("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                "clientInfo": {"name": "fixture", "version": "1"}})
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        self.proc.stdin.flush()

    def close(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=3)
        finally:
            if self.proc.poll() is None:
                self.proc.kill()
                self.proc.wait(timeout=2)
            self.reader.join(timeout=1)
            stderr = self.proc.stderr.read()
            self.proc.stdout.close()
            self.proc.stderr.close()
            self.assertNotIn("fixture_only", stderr)

    def tearDown(self):
        try:
            self.close()
        finally:
            self.server.shutdown()
            self.server.server_close()
            self.thread.join(timeout=2)
            self.tmp.cleanup()

    def send(self, value):
        self.proc.stdin.write(json.dumps(value) + "\n")
        self.proc.stdin.flush()

    def begin_rpc(self, method, params):
        self.serial += 1
        self.send({"jsonrpc": "2.0", "id": self.serial, "method": method, "params": params})
        return self.serial

    def rpc(self, method, params):
        identity = self.begin_rpc(method, params)
        result = self.responses.get(timeout=5)
        self.assertEqual(result["id"], identity)
        return result

    def control(self, key, action, ttl=1500):
        return self.rpc("tools/call", {"name": "tv_control", "arguments": {
            "request_id": key, "device": "media_screen", "action": action,
            "expires_at_unix_ms": int(time.time() * 1000) + ttl}})["result"]

    def test_track_json_body_exact_evidence_and_durable_dedup(self):
        action = {"type": "select_audio_track", "item_id": "episode", "source_id": "source", "index": 1}
        result = self.control("audio_once", action)
        self.assertEqual(result["structuredContent"]["status"], "verified", result)
        self.assertEqual(result["structuredContent"]["observed"]["audio_index"], 1)
        self.assertEqual(self.server.state["posts"], [{"path": "/Sessions/session/Command",
                         "body": self.server.state["posts"][0]["body"]}])
        self.assertEqual(json.loads(self.server.state["posts"][0]["body"]),
                         {"Name": "SetAudioStreamIndex", "Arguments": {"Index": "1"}})
        self.close()
        self.start()
        self.assertTrue(self.control("audio_once", action)["structuredContent"]["cached"])
        self.assertEqual(len(self.server.state["posts"]), 1)
        action["index"] = 0
        self.assertEqual(self.control("audio_once", action)["structuredContent"]["status"], "invalid")
        self.assertEqual(len(self.server.state["posts"]), 1)

    def test_null_subtitle_is_unknown_and_off_requires_a_command(self):
        action = {"type": "select_subtitle_track", "item_id": "episode", "source_id": "source", "index": -1}
        result = self.control("subtitles_off", action)
        self.assertEqual(result["structuredContent"]["status"], "verified", result)
        self.assertEqual(result["structuredContent"]["observed"]["subtitle_index"], -1)
        self.assertEqual(len(self.server.state["posts"]), 1)

    def test_relative_seek_uses_one_absolute_target_and_preserves_pause(self):
        action = {"type": "seek_relative", "item_id": "episode", "source_id": "source", "delta_ms": -1000}
        result = self.control("seek_once", action)
        self.assertEqual(result["structuredContent"]["status"], "verified", result)
        self.assertEqual(result["structuredContent"]["observed"]["playback"], "paused")
        self.assertEqual(self.server.state["position"], 40_000_000)
        self.assertEqual(len(self.server.state["posts"]), 1)
        self.assertTrue(self.control("seek_once", action)["structuredContent"]["cached"])
        self.assertEqual(self.server.state["position"], 40_000_000)

    def test_ignored_small_relative_seek_is_uncertain_not_verified_by_tolerance(self):
        self.server.state["ignore_seek"] = True
        action = {"type": "seek_relative", "item_id": "episode", "source_id": "source", "delta_ms": -50}
        self.assert_ignored_after_verification("ignored_seek", action)
        self.assertEqual(self.server.state["position"], 50_000_000)

    def test_playing_relative_seek_refuses_before_post_despite_natural_progress(self):
        self.server.state["playing"] = True
        for delta in (-50, 50, -1000, 1000):
            action = {"type": "seek_relative", "item_id": "episode", "source_id": "source", "delta_ms": delta}
            result = self.control("playing_seek_" + str(delta).replace("-", "minus"), action)
            self.assertEqual(result["structuredContent"]["status"], "unsupported", result)
            self.assertEqual(result["structuredContent"]["code"], "relative_seek_playing_unverifiable")
            self.assertEqual(self.server.state["posts"], [])
    def test_missing_capability_and_wrong_track_refuse_before_post(self):
        self.server.state["commands"] = []
        action = {"type": "select_audio_track", "item_id": "episode", "source_id": "source", "index": 1}
        result = self.control("unsupported", action)
        self.assertEqual(result["structuredContent"]["status"], "unsupported", result)
        self.assertEqual(self.server.state["posts"], [])
        self.server.state["commands"] = ["SetAudioStreamIndex"]
        action["index"] = 4  # Subtitle index cannot be reused as audio.
        result = self.control("wrong_type", action)
        self.assertTrue(result["isError"], result)
        self.assertEqual(self.server.state["posts"], [])

    def test_ack_without_matching_readback_is_uncertain_never_repaired(self):
        self.server.state["ignore"] = True
        action = {"type": "select_audio_track", "item_id": "episode", "source_id": "source", "index": 1}
        self.assert_ignored_after_verification("ignored", action)
        self.assertEqual(self.server.state["audio"], 0)

    def assert_ignored_after_verification(self, key, action):
        barrier = self.server.verification_barrier = VerificationBarrier()
        identity = self.serial + 1
        cancel = lambda: self.send({"jsonrpc": "2.0", "method": "notifications/cancelled",
                                   "params": {"requestId": identity, "reason": "ignored readback"}})
        # This is a post-dispatch evidence test, not a real-time startup deadline
        # test. Block the third snapshot only after two unchanged reads were
        # processed; cancellation must never make either read count as verified.
        with barrier.cancel_on_exit(cancel):
            self.begin_rpc("tools/call", {"name": "tv_control", "arguments": {
                "request_id": key, "device": "media_screen", "action": action,
                "expires_at_unix_ms": int(time.time() * 1000) + 10000}})
            self.assertTrue(barrier.blocked.wait(timeout=8),
                            f"verification not reached: {barrier.post_count} POSTs, {barrier.session_reads} reads")
            self.assertEqual(barrier.session_reads, 3)
            self.assertEqual(barrier.post_count, 1)
            self.assertEqual(len(self.server.state["posts"]), 1)
            cancel()
            result = self.responses.get(timeout=5)
            self.assertEqual(result["id"], identity)
            value = result["result"]["structuredContent"]
            self.assertEqual(value["status"], "outcome_unknown", value)
            self.assertEqual(value["code"], "cancelled")
            self.assertFalse(value["cached"])
            replay = self.control(key, action)["structuredContent"]
            self.assertEqual(replay["status"], "outcome_unknown")
            self.assertTrue(replay["cached"])
            self.assertEqual(len(self.server.state["posts"]), 1)


def load_tests(loader, tests, pattern):
    # Reuse the fake ADB harness, without duplicating all inherited base tests.
    return unittest.TestSuite(loader.loadTestsFromNames([
        cls.__name__ + "." + name for cls in (Volume, Media)
        for name in cls.__dict__ if name.startswith("test_")
    ], module=sys.modules[__name__]))


if __name__ == "__main__":
    unittest.main()
