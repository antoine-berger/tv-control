#!/usr/bin/env python3
"""Execute actual owner shell logic with owned host-Python/tool fakes; no Android."""
import os
from pathlib import Path
import selectors
import shlex
import struct
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from owned_runtime import leader_exited, stop_unreaped_group

INSTANCE = "abcdef0123456789abcdef0123456789"
ARTIFACT = "/data/app/synthetic-fixture/base.apk"
CLOSED = struct.pack(">IIBBB", 7, 0x4E544C31, 1, 0x82, 1)
PAYLOAD = b"synthetic-private-fixture-input\n"


class OwnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="lab-owner-host-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        fake = self.root / "fake-app-process"
        fake.write_text("#!" + sys.executable + "\n" + r'''
import os,struct,sys
if sys.argv[1:] != ['/system/bin','org.tvcontrol.nativeinput.LabSocketClient',
                     'abcdef0123456789abcdef0123456789','10001']:
    raise SystemExit(78)
if os.environ.get('CLASSPATH') != '/data/app/synthetic-fixture/base.apk':
    raise SystemExit(78)
if sys.stdin.buffer.readline() != b'synthetic-private-fixture-input\n':
    raise SystemExit(79)
sys.stdout.buffer.write(struct.pack('>IIBBB',7,0x4e544c31,1,0x82,1))
sys.stdout.buffer.flush()
# Stay alive after CHANNEL_CLOSED: the owner must not report us reaped yet.
if sys.stdin.buffer.readline() != b'release\n':
    raise SystemExit(77)
exit_code=int(os.environ.get('OWNER_FAKE_EXIT','0'))
if exit_code<0:
    os.kill(os.getpid(),-exit_code)
raise SystemExit(exit_code)
''')
        fake.chmod(0o700)
        original = (Path(__file__).parent / "client-owner.sh").read_text()
        self.assertEqual(original.count("/system/bin/app_process"), 1)
        self.assertEqual(original.count("/system/bin/toybox printf"), 1)
        # Substitute only fixed tool paths in the copied test script. Production
        # has no environment/argv override allowing a different executable.
        copied = original.replace("/system/bin/app_process", shlex.quote(str(fake)))
        copied = copied.replace("/system/bin/toybox printf", "/usr/bin/printf")
        self.script = self.root / "owner.sh"
        self.script.write_text(copied)

    def spawn(self, exit_code=0, arguments=None):
        env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C", "OWNER_FAKE_EXIT": str(exit_code)}
        process = subprocess.Popen(["/bin/sh", self.script, *(arguments or [INSTANCE, ARTIFACT, "10001"])],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   env=env, start_new_session=True)
        def cleanup():
            if process.returncode is None:
                stop_unreaped_group(process, grace_seconds=.2)
            for stream in (process.stdin, process.stdout, process.stderr):
                if not stream.closed:
                    stream.close()
        self.addCleanup(cleanup)
        return process

    def exact(self, stream, size):
        result = bytearray()
        deadline = time.monotonic() + 3
        with selectors.DefaultSelector() as poll:
            poll.register(stream, selectors.EVENT_READ)
            while len(result) < size:
                remaining = deadline - time.monotonic()
                self.assertGreater(remaining, 0, "owned fake-process output deadline")
                self.assertTrue(poll.select(remaining), "owned fake-process output deadline")
                chunk = os.read(stream.fileno(), size - len(result))
                self.assertTrue(chunk, "unexpected EOF in fixed owner frame")
                result.extend(chunk)
        return bytes(result)

    def exit(self, process):
        deadline = time.monotonic() + 3
        while not leader_exited(process):
            self.assertLess(time.monotonic(), deadline, "owned fake-process exit deadline")
            time.sleep(.01)
        stop_unreaped_group(process, grace_seconds=0)
        return process.returncode

    def test_foreground_preserves_stdin_and_witness_follows_real_child_exit(self):
        for code in (0, 7, -15):
            with self.subTest(exit_class=code):
                process = self.spawn(code)
                process.stdin.write(PAYLOAD); process.stdin.flush()  # deliberately keep stdin open
                self.assertEqual(self.exact(process.stdout, len(CLOSED)), CLOSED)
                with selectors.DefaultSelector() as poll:
                    poll.register(process.stdout, selectors.EVENT_READ)
                    self.assertFalse(poll.select(.05), "CHANNEL_CLOSED is not a remote reaping proof")
                self.assertFalse(leader_exited(process))
                process.stdin.write(b"release\n"); process.stdin.flush()
                expected = struct.pack(">IIBB", 23, 0x4E544C31, 1, 0x83) + bytes.fromhex(INSTANCE) + bytes([bool(code)])
                self.assertEqual(self.exact(process.stdout, len(expected)), expected)
                self.assertEqual(self.exit(process), code if code>=0 else 128-code)
                self.assertEqual(process.stdout.read(), b"")
                self.assertNotIn(PAYLOAD.strip(), process.stderr.read())

    def test_closing_stdin_after_first_message_is_not_a_successful_protocol(self):
        process = self.spawn()
        process.stdin.write(PAYLOAD); process.stdin.close()
        self.assertEqual(self.exact(process.stdout, len(CLOSED)), CLOSED)
        expected = struct.pack(">IIBB", 23, 0x4E544C31, 1, 0x83) + bytes.fromhex(INSTANCE) + b"\x01"
        self.assertEqual(self.exact(process.stdout, len(expected)), expected)
        self.assertEqual(self.exit(process), 77)

    def test_invalid_public_arguments_never_launch_child_or_emit_witness(self):
        for args in ([INSTANCE + "\n", ARTIFACT, "10001"], [INSTANCE, ARTIFACT, "0"],
                     [INSTANCE, ARTIFACT, "2000"], [INSTANCE, ARTIFACT, "010001"],
                     [INSTANCE, ARTIFACT, "2147483648"], [INSTANCE, ARTIFACT, "10001;false"],
                     [INSTANCE, "/outside/base.apk", "10001"],
                     [INSTANCE, "/data/app/../escape/base.apk", "10001"],
                     [INSTANCE, "/data/app/fixture/$(bad)/base.apk", "10001"]):
            with self.subTest(case=args):
                process = self.spawn(arguments=args)
                self.assertEqual(self.exit(process), 64)
                self.assertEqual(process.stdout.read(), b"")
                self.assertEqual(process.stderr.read(), b"")


if __name__ == "__main__":
    unittest.main()
