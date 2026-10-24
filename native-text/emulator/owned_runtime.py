"""Bounded subprocesses and a directly owned, isolated foreground ADB server."""
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
import time

from adb_diagnostic import capture as capture_adb_diagnostic
from listener_fields import owned_loopback_listener
from process_group import leader_exited, retained_exit_info, stop_unreaped_group

OUTPUT_LIMIT = 128 * 1024


def command(argv, env, timeout=30):
    """Capture locally; terminate descendants before the one reaping operation."""
    if timeout <= 0:
        raise RuntimeError("command deadline expired before process dispatch")
    expires = time.monotonic() + timeout
    with tempfile.TemporaryFile() as output:
        process = subprocess.Popen([str(x) for x in argv], env=env, stdin=subprocess.DEVNULL,
                                   stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            while not leader_exited(process):
                if os.fstat(output.fileno()).st_size > OUTPUT_LIMIT:
                    raise RuntimeError("bounded fixture output exceeded")
                if time.monotonic() >= expires:
                    raise subprocess.TimeoutExpired(str(argv[0]), timeout)
                time.sleep(0.02)
            if os.fstat(output.fileno()).st_size > OUTPUT_LIMIT:
                raise RuntimeError("bounded fixture output exceeded")
        finally:
            stop_unreaped_group(process, grace_seconds=0)
        if process.returncode:
            raise subprocess.CalledProcessError(process.returncode, str(argv[0]))
        output.seek(0)
        data = output.read(OUTPUT_LIMIT + 1)
        if len(data) > OUTPUT_LIMIT:
            raise RuntimeError("bounded fixture output exceeded")
        return data.decode("utf-8", errors="strict")


def reserve_port(port=0):
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        sock.bind(("127.0.0.1", port))
        return sock
    except BaseException:
        sock.close()
        raise


class Ports:
    def __init__(self):
        self.server = reserve_port()
        self.adb_port = self.server.getsockname()[1]
        self.console_sockets = []
        # Outside the default ADB scan of the first sixteen emulator pairs.
        # This is a documented emulator port range, not a device identity.
        for port in range(5602, 5683, 2):
            try:
                console = reserve_port(port)
                try:
                    transport = reserve_port(port + 1)
                except BaseException:
                    console.close()
                    raise
            except OSError:
                continue
            self.console_port = port
            self.console_sockets = [console, transport]
            break
        if not self.console_sockets:
            self.close()
            raise RuntimeError("no free bounded emulator console/ADB pair")

    def release_server(self):
        self.server.close()

    def release_console(self):
        for sock in self.console_sockets:
            sock.close()

    def close(self):
        self.release_server()
        self.release_console()


def isolated_environment(sdk, work, server_port, console_port, inherited=None):
    # Do not inherit a serial, a vendor key, a global socket, Android home or
    # tracing options that could reveal input or access unrelated devices.
    # Build a small explicit environment instead of leaking account tokens,
    # dynamic-loader overrides, global SDK paths or shell configuration.
    env = {"PATH": str(sdk / "platform-tools") + os.pathsep + os.defpath,
           "LANG": "C", "LC_ALL": "C", "TMPDIR": str(work / "tmp")}
    home = work / "home"
    return dict(env, HOME=str(home), ANDROID_USER_HOME=str(home / ".android"),
                ANDROID_SDK_ROOT=str(sdk), ANDROID_HOME=str(sdk),
                ANDROID_AVD_HOME=str(work), ANDROID_EMULATOR_HOME=str(work),
                ANDROID_SDK_HOME=str(home), ANDROID_SERIAL=f"emulator-{console_port}",
                ADB_SERVER_SOCKET=f"tcp:127.0.0.1:{server_port}",
                ANDROID_ADB_SERVER_ADDRESS="127.0.0.1", ANDROID_ADB_SERVER_PORT=str(server_port),
                ADB_USB="0", ADB_EMU="0", ADB_MDNS="0", ADB_MDNS_AUTO_CONNECT="0",
                ADB_LOCAL_TRANSPORT_MAX_PORT="0", ADB_REJECT_KILL_SERVER="1",
                ADB_VENDOR_KEYS=str(home / "empty-vendor-keys"))


def host_query(port, query, limit=4096):
    with socket.create_connection(("127.0.0.1", port), timeout=1) as sock:
        sock.settimeout(1)
        request = query.encode("ascii")
        sock.sendall(f"{len(request):04x}".encode("ascii") + request)
        def exact(count):
            result = b""
            while len(result) < count:
                data = sock.recv(count - len(result))
                if not data:
                    raise RuntimeError("private ADB response closed")
                result += data
            return result
        if exact(4) != b"OKAY":
            raise RuntimeError("private ADB query refused")
        raw_size = exact(4)
        if not re.fullmatch(b"[0-9a-fA-F]{4}", raw_size):
            raise RuntimeError("private ADB size malformed")
        size = int(raw_size, 16)
        if size > limit:
            raise RuntimeError("private ADB response exceeded bound")
        return exact(size).decode("ascii", errors="strict")


def owns_listener(pid, port, env):
    """Read-only ownership proof; never trust a version-compatible foreign daemon."""
    if sys.platform == "darwin":
        try:
            output = command(["/usr/sbin/lsof", "-nP", "-a", "-p", str(pid),
                              f"-iTCP:{port}", "-sTCP:LISTEN", "-Fpn"], env, timeout=3)
        except subprocess.CalledProcessError:
            return False
        return owned_loopback_listener(output, pid, port)
    if sys.platform == "linux":
        root = Path(f"/proc/{pid}")
        inodes = {os.readlink(path)[8:-1] for path in (root / "fd").iterdir()
                  if os.readlink(path).startswith("socket:[")}
        wanted = f"0100007F:{port:04X}"
        rows = (root / "net/tcp").read_text().splitlines()[1:]
        return any((parts := row.split())[1] == wanted and parts[3] == "0A"
                   and parts[9] in inodes for row in rows)
    raise RuntimeError("listener ownership proof unavailable on this platform")


def sdk_processes(sdk, env):
    """Detection only. Never signal a PID obtained by enumeration."""
    if sys.platform == "linux":
        return linux_sdk_processes(sdk)
    result = []
    output = command(["/bin/ps", "-axo", "pid=,ppid=,comm="], env, timeout=5)
    for row in output.splitlines():
        values = row.strip().split(None, 2)
        if len(values) == 3 and values[2].startswith(str(sdk.resolve()) + os.sep):
            result.append((int(values[0]), int(values[1])))
    return result


def linux_sdk_processes(sdk, proc_root=Path("/proc")):
    # Linux ps comm is a short basename. Read the current user's executable
    # links instead; never treat a basename match as SDK provenance/ownership.
    result = []
    for entry in proc_root.iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            status = (entry / "status").read_text()
            uid = re.search(r"^Uid:\s+(\d+)", status, re.M)
            if uid is None or int(uid[1]) != os.getuid():
                continue
            executable = os.readlink(entry / "exe")
        except FileNotFoundError:
            continue
        except PermissionError as error:
            raise RuntimeError("owned-user SDK process observation unavailable") from error
        if executable.startswith(str(sdk.resolve()) + os.sep):
            parent = re.search(r"^PPid:\s+(\d+)", status, re.M)
            result.append((int(entry.name), int(parent[1]) if parent else 0))
    return result


def port_closed(port):
    try:
        sock = socket.create_connection(("127.0.0.1", port), timeout=.2)
    except ConnectionRefusedError:
        return True
    else:
        sock.close()
        return False


class PrivateAdb:
    def __init__(self, adb, ports, env, diagnostic_directory=None):
        self.adb, self.ports, self.env = adb, ports, env
        self.process = None
        self.log = None
        self.deadline = None
        self.startup_stage = "not_started"
        self.startup_diagnostic = None
        self.diagnostic_directory = diagnostic_directory
        self.serial = f"emulator-{ports.console_port}"

    def start(self):
        try:
            self._start()
        except BaseException:
            self.collect_startup_diagnostic(failed=True)
            raise

    def _start(self):
        self.startup_stage = "version"
        version = command([self.adb, "version"], self.env, timeout=10)
        match = re.search(r"^Android Debug Bridge version 1\.0\.(\d+)\s*$", version, re.M)
        if not match:
            raise RuntimeError("private ADB protocol version unavailable")
        self.protocol = int(match[1])
        self.log = tempfile.TemporaryFile()
        self.startup_stage = "launch"
        self.ports.release_server()
        # SDK socket_spec_listen recognizes the literal "localhost" as IPv4
        # loopback; numeric 127.0.0.1 is supported for connect, not listen.
        # This branch calls network_loopback_server directly, without DNS.
        self.process = subprocess.Popen([str(self.adb), "-L", f"tcp:localhost:{self.ports.adb_port}",
                                         "server", "nodaemon"], env=self.env,
                                        stdin=subprocess.DEVNULL, stdout=self.log,
                                        stderr=subprocess.STDOUT, start_new_session=True)
        self.startup_stage = "readiness"
        until = time.monotonic() + 10
        while True:
            if leader_exited(self.process):
                raise RuntimeError("owned foreground ADB exited before readiness")
            try:
                self.guard()
                break
            except (OSError, RuntimeError):
                if time.monotonic() >= until:
                    raise RuntimeError("owned foreground ADB readiness deadline exceeded")
                time.sleep(0.05)
        if host_query(self.ports.adb_port, "host:devices").strip():
            raise RuntimeError("private ADB unexpectedly contains an existing target")
        self.startup_stage = "ready"

    def collect_startup_diagnostic(self, failed=False):
        if self.startup_diagnostic is not None:
            return
        try:
            diagnostic = capture_adb_diagnostic(self.log, self.diagnostic_directory if failed else None)
        except (OSError, ValueError):
            diagnostic = {"capture": "unavailable", "categories": ["UNKNOWN"], "source_frames": []}
            if failed and self.diagnostic_directory is not None:
                diagnostic["raw_save_failed"] = True
        diagnostic["stage"] = self.startup_stage
        try:
            diagnostic["process"] = retained_exit_info(self.process) if self.process else {"state": "not_started"}
        except (OSError, RuntimeError):
            diagnostic["process"] = {"state": "ownership_unknown"}
        self.startup_diagnostic = diagnostic

    def guard(self):
        if self.process is None or leader_exited(self.process):
            raise RuntimeError("owned foreground ADB is not live; no client dispatch")
        if os.fstat(self.log.fileno()).st_size > OUTPUT_LIMIT:
            raise RuntimeError("private ADB diagnostics exceeded bound")
        if not owns_listener(self.process.pid, self.ports.adb_port, self.env):
            raise RuntimeError("private ADB listener is not owned and loopback-only")
        version = host_query(self.ports.adb_port, "host:version", limit=4)
        if not re.fullmatch(r"[0-9a-fA-F]{4}", version) or int(version, 16) != self.protocol:
            raise RuntimeError("private ADB version differs; no client dispatch")
        if leader_exited(self.process):
            raise RuntimeError("owned foreground ADB stopped during guard")
        listing = host_query(self.ports.adb_port, "host:devices")
        for row in listing.splitlines():
            columns = row.split("\t")
            if len(columns) != 2 or columns[0] != self.serial or columns[1] not in ("device", "offline", "unauthorized"):
                raise RuntimeError("private ADB contains an unexpected target; no client dispatch")

    def target(self, *words, timeout=30, cleanup=False):
        until = time.monotonic() + timeout
        if self.deadline is not None and not cleanup:
            until = min(until, self.deadline)
        self.guard()
        remaining = until - time.monotonic()
        if remaining <= 0:
            raise RuntimeError("target deadline expired during preflight; no client dispatch")
        # The server/port and serial are never caller-supplied words. A startup
        # race in the SDK client remains possible; postchecks must not hide it.
        result = command([self.adb, "-H", "127.0.0.1", "-P", str(self.ports.adb_port),
                          "-s", self.serial, *words], self.env, remaining)
        self.guard()
        return result

    def close(self):
        # Preserve fixed-category evidence before the temporary SDK log closes.
        # No raw line, arbitrary SDK message, path, key or credential is returned.
        self.collect_startup_diagnostic()
        try:
            if self.process is not None:
                stop_unreaped_group(self.process, grace_seconds=1)
        finally:
            if self.log is not None:
                self.log.close()
        # This test cannot kill a daemon that escaped its owned leader/session.
        # A remaining listener is an explicit cleanup failure, never success.
        if not port_closed(self.ports.adb_port):
            raise RuntimeError("private ADB listener remains after owned cleanup")
        if self.startup_diagnostic.get("raw_save_failed"):
            raise RuntimeError("private ADB startup diagnostic preservation failed")
