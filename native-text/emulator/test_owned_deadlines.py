"""Absolute deadlines with socket/command fakes; no socket or SDK is executed."""
import io
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import owned_runtime as runtime


class Clock:
    now = 100
    def __call__(self):
        return self.now


class FakeSocket:
    def __init__(self, clock, send_cost=0, receive_cost=.2):
        self.clock, self.send_cost, self.receive_cost = clock, send_cost, receive_cost
        self.timeouts, self.reads = [], 0
        self.data = bytearray(b'OKAY0004abcd')
    def __enter__(self):
        return self
    def __exit__(self, *args):
        pass
    def settimeout(self, value):
        self.timeouts.append(value)
    def sendall(self, request):
        self.clock.now += self.send_cost
    def recv(self, size):
        self.reads += 1
        self.clock.now += self.receive_cost
        result = bytes(self.data[:1])
        del self.data[:1]
        return result


class Deadlines(unittest.TestCase):
    def test_slow_bytes_do_not_renew_the_absolute_query_deadline(self):
        clock = Clock()
        sock = FakeSocket(clock)
        with patch.object(runtime.time, 'monotonic', clock), patch.object(runtime.socket, 'create_connection', return_value=sock) as connect:
            with self.assertRaisesRegex(RuntimeError, 'absolute deadline'):
                runtime.host_query(49152, 'host:version', limit=4, deadline=100.5)
        self.assertEqual(connect.call_args.kwargs['timeout'], .5)
        self.assertEqual(sock.reads, 3)
        self.assertEqual(sock.timeouts, sorted(sock.timeouts, reverse=True))
        self.assertLess(sock.timeouts[-1], .11)

    def test_send_consumes_same_deadline_and_prevents_any_receive(self):
        clock = Clock()
        sock = FakeSocket(clock, send_cost=1)
        with patch.object(runtime.time, 'monotonic', clock), patch.object(runtime.socket, 'create_connection', return_value=sock):
            with self.assertRaisesRegex(RuntimeError, 'absolute deadline'):
                runtime.host_query(49152, 'host:version', deadline=100.5)
        self.assertEqual(sock.reads, 0)

    def test_expired_query_and_lsof_do_not_dispatch(self):
        clock = Clock()
        with patch.object(runtime.time, 'monotonic', clock), patch.object(runtime.socket, 'create_connection') as connect, patch.object(runtime, 'command') as command:
            with self.assertRaises(RuntimeError):
                runtime.host_query(49152, 'host:version', deadline=99)
            with self.assertRaises(RuntimeError):
                runtime.owns_listener(123, 49152, {}, deadline=99)
            connect.assert_not_called()
            command.assert_not_called()

    def test_lsof_uses_remaining_budget_and_rechecks_after_command(self):
        clock = Clock()
        def command(*args, **kwargs):
            self.assertEqual(kwargs['timeout'], .5)
            clock.now = 101
            return 'p123\nf5\nn127.0.0.1:49152\n'
        with patch.object(runtime.time, 'monotonic', clock), patch.object(runtime.sys, 'platform', 'darwin'), patch.object(runtime, 'command', side_effect=command):
            with self.assertRaises(RuntimeError):
                runtime.owns_listener(123, 49152, {}, deadline=100.5)

    def test_guard_propagates_one_deadline_and_requires_one_ready_target(self):
        adb = runtime.PrivateAdb(Path('/owned/fake-adb'), SimpleNamespace(adb_port=49152, console_port=5630), {})
        adb.process, adb.log, adb.protocol = SimpleNamespace(pid=123), None, 41
        # fstat receives only a fake descriptor here; no file or listener is read.
        adb.log = SimpleNamespace(fileno=lambda: 12)
        for listing, valid in [(adb.serial + '\tdevice\n', True), ('', False),
                               (adb.serial + '\toffline\n', False), (adb.serial + '\tdevice\nother\tdevice\n', False)]:
            def query(port, name, **kwargs):
                self.assertEqual(kwargs['deadline'], 100.5)
                return '0029' if name == 'host:version' else listing
            with self.subTest(listing=listing), patch.object(runtime.time, 'monotonic', Clock()), \
                 patch.object(runtime, 'leader_exited', return_value=False), patch.object(runtime.os, 'fstat', return_value=SimpleNamespace(st_size=0)), \
                 patch.object(runtime, 'owns_listener', return_value=True) as listener, patch.object(runtime, 'host_query', side_effect=query):
                if valid:
                    adb.guard(100.5, require_device=True)
                else:
                    with self.assertRaises(RuntimeError):
                        adb.guard(100.5, require_device=True)
                self.assertEqual(listener.call_args.kwargs['deadline'], 100.5)


if __name__ == '__main__':
    unittest.main()
