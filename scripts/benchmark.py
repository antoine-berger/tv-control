#!/usr/bin/env python3
"""Host-only measurements. Never connects to a device or reads a real config."""
import argparse
import hashlib
import json
import os
import pathlib
import platform
import statistics
import subprocess
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from test_mcp import Fixture

parser = argparse.ArgumentParser()
parser.add_argument("binary")
parser.add_argument("--compiler-version", default="not_recorded")
parser.add_argument("--cli-samples", type=int, default=50)
parser.add_argument("--rpc-samples", type=int, default=1000)
parser.add_argument("--idle-seconds", type=float, default=5)
parser.add_argument("--fixture-samples", type=int, default=20)
args = parser.parse_args()
assert args.cli_samples > 0 and args.rpc_samples > 0 and args.idle_seconds >= 1
root = pathlib.Path(__file__).resolve().parent.parent
binary = pathlib.Path(args.binary).resolve()
config = root / "examples/config.json"

def summary(values):
    ordered = sorted(values)
    return {"samples": len(values), "p50_ms": statistics.median(values),
            "p95_ms": ordered[max(0, (95 * len(values) + 99) // 100 - 1)], "max_ms": max(values)}

def launch():
    return subprocess.Popen([str(binary), "mcp", str(config)], stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

def call(proc, value):
    proc.stdin.write(json.dumps(value) + "\n")
    proc.stdin.flush()
    line = proc.stdout.readline()
    if not line:
        raise RuntimeError("MCP process exited")
    result = json.loads(line)
    assert "error" not in result, result
    return result

def initialize(proc):
    call(proc, {"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"benchmark","version":"1"}}})
    proc.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
    proc.stdin.flush()

def close(proc):
    proc.stdin.close()
    proc.wait(timeout=3)
    proc.stdout.close()
    proc.stderr.close()

def reap_with_usage(proc):
    proc.stdin.close()
    _, status, usage = os.wait4(proc.pid, 0)
    proc.returncode = os.waitstatus_to_exitcode(status)
    proc.stdout.close()
    proc.stderr.close()
    return usage

cold = []
for _ in range(args.cli_samples):
    started = time.perf_counter_ns()
    subprocess.run([str(binary), "validate", str(config)], stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL, check=True, timeout=3)
    cold.append((time.perf_counter_ns() - started) / 1_000_000)

proc = launch()
initialize(proc)
warm = []
for i in range(args.rpc_samples):
    started = time.perf_counter_ns()
    call(proc, {"jsonrpc":"2.0","id":i+2,"method":"ping"})
    warm.append((time.perf_counter_ns() - started) / 1_000_000)
close(proc)

idle_started = time.perf_counter()
proc = launch()
initialize(proc)
rss = []
for _ in range(5):
    rss.append(int(subprocess.check_output(["ps", "-o", "rss=", "-p", str(proc.pid)], text=True).strip()) * 1024)
    time.sleep(args.idle_seconds / 5)
usage = reap_with_usage(proc)
idle_wall = time.perf_counter() - idle_started
cpu = usage.ru_utime + usage.ru_stime

Fixture.paused = False
Fixture.slow = False
server = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
threading.Thread(target=server.serve_forever, daemon=True).start()
fixture_latencies = []
with tempfile.TemporaryDirectory() as temporary:
    cfg = json.loads(config.read_text())
    cfg["servers"]["library"]["base_url"] = f"http://127.0.0.1:{server.server_port}"
    cfg["servers"]["library"]["allow_insecure_http"] = True
    fixture_path = pathlib.Path(temporary) / "fixture.json"
    fixture_path.write_text(json.dumps(cfg))
    environment = os.environ.copy()
    environment["TV_LIBRARY_TOKEN"] = "fixture_only_not_a_credential"
    proc = subprocess.Popen([str(binary), "mcp", str(fixture_path)], stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=environment)
    initialize(proc)
    for i in range(args.fixture_samples):
        started = time.perf_counter_ns()
        response = call(proc, {"jsonrpc":"2.0","id":i+2,"method":"tools/call","params":{
            "name":"tv_control","arguments":{"request_id":f"fixture_{i}","device":"media_screen",
                "action":{"type":"pause" if i % 2 == 0 else "resume"},
                "expires_at_unix_ms":int(time.time()*1000)+8000}}})
        assert response["result"]["structuredContent"]["status"] == "verified", response
        fixture_latencies.append((time.perf_counter_ns()-started)/1_000_000)
    fixture_usage = reap_with_usage(proc)
server.shutdown()
server.server_close()
result = {
    "scope": "host_only_no_device_no_chat_no_adb_daemon_no_tunnel",
    "system": platform.system(), "machine": platform.machine(), "compiler": args.compiler_version,
    "binary_bytes": binary.stat().st_size, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
    "cold_cli_validation": summary(cold), "warm_stdio_ping": summary(warm),
    "idle": {"rss_samples_bytes": rss, "rss_max_bytes": max(rss),
             "child_peak_rss_bytes": usage.ru_maxrss * (1 if platform.system() == "Darwin" else 1024),
             "wall_seconds": idle_wall, "cpu_seconds": cpu, "cpu_percent_one_core": 100 * cpu / idle_wall,
             "cpu_scope": "server_process_startup_initialization_idle_shutdown_average"},
    "end_to_end_user_to_tv": "not_measured", "router_runtime": "not_tested"
}
result["loopback_jellyfin_mcp"] = summary(fixture_latencies)
result["loopback_jellyfin_mcp"]["server_peak_rss_bytes"] = fixture_usage.ru_maxrss * (1 if platform.system() == "Darwin" else 1024)
result["loopback_jellyfin_mcp"]["scope"] = "host_stdio_to_loopback_http_fixture_postcondition_no_physical_media"
print(json.dumps(result, indent=2))
