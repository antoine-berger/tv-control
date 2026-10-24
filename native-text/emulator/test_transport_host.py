import struct
import unittest
from collections import deque
from transport_host import Host
from test_transport_bundle import PUBLIC_INSTANCE, final, ready
from test_transport_wire import closed, owner, reference


class Clock:
    def __init__(self):
        self.now = 0

    def __call__(self):
        return self.now

    def wait(self, children, timeout):
        self.now += timeout


class FakeChild:
    def __init__(self, launcher, name):
        self.launcher, self.name = launcher, name
        self.output = deque()
        self.errors = deque()
        self.pending = bytearray()
        self.received = bytearray()
        self.closed_input = False
        self.eof = {'stdout': False, 'stderr': False}
        self.done = False
        self.cleanups = 0

    def queue(self, data):
        if self.closed_input:
            raise RuntimeError('synthetic private exception')
        self.pending.extend(data)

    def pending_bytes(self):
        return len(self.pending)

    def flush(self):
        count = min(self.launcher.write_limit, len(self.pending))
        self.received.extend(self.pending[:count])
        del self.pending[:count]
        while len(self.received) >= 4:
            length, = struct.unpack('>I', self.received[:4])
            if len(self.received) < length + 4:
                break
            command = bytes(self.received[:length + 4])
            del self.received[:length + 4]
            self.launcher.command(command)

    def read(self, name):
        items = self.output if name == 'stdout' else self.errors
        if items:
            return items.popleft()
        if self.done and not self.eof[name]:
            self.eof[name] = True
            return b''
        return None

    def close_input(self):
        if self.name == 'client' and self.launcher.mode == 'delayed_owner' and not self.launcher.witness_emitted:
            raise AssertionError('stdin closed before remote-owner witness')
        self.closed_input = True
        self.pending.clear()
        if self.name == 'client' and self.launcher.mode == 'cancel' and not self.done:
            self.output.append(closed() + owner())
            self.done = True

    def exited(self):
        return self.done and not self.output

    def exit_ok(self):
        return self.launcher.mode != 'local_nonzero'

    def cleanup(self):
        self.cleanups += 1
        self.done = True
        if self.launcher.mode == 'cleanup_raises':
            raise RuntimeError('PRIVATE_SYNTHETIC_CANARY')
        return self.launcher.mode != 'cleanup_false'


class Launcher:
    def __init__(self, mode='valid', write_limit=32768):
        self.mode, self.write_limit = mode, write_limit
        self.channels = []
        self.guards = 0
        self.commands = []
        self.started_clients = 0
        self.witness_emitted = False

    def guard(self):
        self.guards += 1
        if self.mode == 'guard_false':
            return False
        if self.mode == 'guard_failure' or self.mode == 'changed_identity' and self.guards == 3:
            raise RuntimeError('PRIVATE_SYNTHETIC_CANARY')

    def instrumentation(self, instance):
        self.am = FakeChild(self, 'am')
        self.channels.append(self.am)
        if self.mode == 'start_raises':
            raise RuntimeError('PRIVATE_SYNTHETIC_CANARY')
        if self.mode == 'stale_ready':
            self.am.output.append(ready('f' * 32))
        elif self.mode == 'no_ready':
            self.am.output.append(final())
            self.am.done = True
        else:
            self.am.output.append(ready())
        return self.am

    def client(self, instance):
        self.started_clients += 1
        self.peer = FakeChild(self, 'client')
        self.channels.append(self.peer)
        if self.mode == 'client_start_raises':
            raise RuntimeError('PRIVATE_SYNTHETIC_CANARY')
        return self.peer

    def owned(self):
        return tuple(self.channels)

    def command(self, command):
        opcode = command[9]
        self.commands.append(opcode)
        if opcode == 1:
            self.peer.output.append(reference())
            if self.mode == 'forged_verified_before_commit':
                self.am.output.append(final())
                self.am.done = True
                self.peer.output.append(closed() + owner())
                self.peer.done = True
        elif opcode == 2:
            self.am.output.append(final())
            self.am.done = True
            if self.mode == 'no_bundle':
                self.am.output.clear()
            elif self.mode == 'double_bundle':
                self.am.output.append(final())
            elif self.mode == 'stale_bundle':
                self.am.output.clear()
                self.am.output.append(final(tv_native_lab_instance_v1='f' * 32))
            elif self.mode == 'raw_output':
                self.am.output.append(b'PRIVATE_SYNTHETIC_CANARY\n')
            elif self.mode == 'oversized':
                self.am.output.append(b'x' * 8193)
            if self.mode == 'cancel':
                return
            if self.mode == 'survivor':
                return
            self.peer.output.append(closed())
            if self.mode == 'delayed_owner':
                return
            if self.mode != 'no_remote_owner':
                self.peer.output.append(owner(status=1 if self.mode == 'remote_nonzero' else 0))
            self.peer.done = True


class Supervision(unittest.TestCase):
    def run_case(self, mode='valid', write_limit=32768, cancelled=None, budget=.5):
        launcher = Launcher(mode, write_limit)
        clock = Clock()
        host = Host(launcher, clock=clock, wait=clock.wait,
                    entropy=lambda: PUBLIC_INSTANCE, budget=budget)
        outcome = host.run('PRIVATE_SYNTHETIC_CANARY', cancelled or (lambda: False))
        return launcher, clock, host, outcome

    def test_valid_requires_both_remote_proofs_and_keeps_stdin_until_witness(self):
        launcher, _, _, outcome = self.run_case()
        self.assertEqual(outcome.status, 'VERIFIED')
        self.assertTrue(outcome.qualified)
        self.assertEqual(outcome.client_cleanup, 'REAPED')
        self.assertEqual(launcher.commands, [1, 2])
        self.assertTrue(launcher.peer.closed_input)
        self.assertEqual([child.cleanups for child in launcher.channels], [1, 1])
        self.assertNotIn('PRIVATE_SYNTHETIC_CANARY', str(outcome.public()))

    def test_final_bundle_does_not_close_stdin_before_remote_owner_witness(self):
        launcher, clock = Launcher('delayed_owner'), Clock()
        def wait(children, timeout):
            clock.wait(children, timeout)
            if clock.now >= .06 and not launcher.witness_emitted:
                launcher.witness_emitted = True
                launcher.peer.output.append(owner())
                launcher.peer.done = True
        result = Host(launcher, clock=clock, wait=wait, entropy=lambda: PUBLIC_INSTANCE, budget=.5).run('fixture')
        self.assertEqual(result.status, 'VERIFIED')
        self.assertTrue(launcher.witness_emitted)

    def test_verified_claim_without_a_flushed_commit_is_invalid(self):
        _, _, _, result = self.run_case('forged_verified_before_commit')
        self.assertEqual(result.status, 'UNKNOWN')
        self.assertFalse(result.qualified)

    def test_short_writes_complete_one_frame_without_replay(self):
        launcher, _, _, outcome = self.run_case(write_limit=3, budget=2)
        self.assertEqual(outcome.status, 'VERIFIED')
        self.assertEqual(launcher.commands, [1, 2])

    def test_exit_zero_or_channel_closed_never_substitutes_for_remote_proof(self):
        for mode in ['no_bundle', 'no_remote_owner', 'local_nonzero', 'remote_nonzero', 'survivor']:
            with self.subTest(mode=mode):
                launcher, _, _, outcome = self.run_case(mode)
                self.assertEqual(outcome.status, 'UNKNOWN')
                self.assertFalse(outcome.qualified)
                self.assertEqual(outcome.public()['cleanup'], 'UNVERIFIED')
                self.assertEqual(launcher.commands.count(2), 1)
                self.assertTrue(all(child.cleanups == 1 for child in launcher.channels))

    def test_stale_duplicate_raw_or_oversized_bundle_never_qualifies(self):
        for mode in ['stale_ready', 'no_ready', 'double_bundle', 'stale_bundle', 'raw_output', 'oversized']:
            with self.subTest(mode=mode):
                _, _, _, outcome = self.run_case(mode)
                self.assertEqual(outcome.status, 'UNKNOWN')
                self.assertFalse(outcome.qualified)
                self.assertNotIn('PRIVATE_SYNTHETIC_CANARY', str(outcome.public()))

    def test_cancellation_is_sticky_despite_verified_bundle_and_later_owner_proof(self):
        launcher = Launcher('cancel')
        clock = Clock()
        host = Host(launcher, clock=clock, wait=clock.wait, entropy=lambda: PUBLIC_INSTANCE, budget=.5)
        result = host.run('fixture', cancelled=lambda: clock.now >= .06)
        self.assertEqual(result.status, 'UNKNOWN')
        self.assertEqual(result.reason, 'HOST_CANCELLED')
        self.assertEqual(launcher.commands, [1, 2])
        self.assertFalse(result.qualified)

    def test_local_cleanup_failure_and_partial_launch_are_not_hidden(self):
        for mode in ['cleanup_raises', 'cleanup_false', 'start_raises', 'client_start_raises']:
            with self.subTest(mode=mode):
                launcher, _, _, result = self.run_case(mode)
                self.assertEqual(result.status, 'UNKNOWN')
                self.assertFalse(result.qualified)
                self.assertTrue(all(child.cleanups == 1 for child in launcher.channels))
                self.assertNotIn('PRIVATE_SYNTHETIC_CANARY', str(result.public()))

    def test_failed_or_false_guard_never_starts_a_child(self):
        for mode in ('guard_false', 'guard_failure'):
            launcher, _, _, result = self.run_case(mode)
            self.assertEqual(result.status, 'REFUSED')
            self.assertEqual(result.reason, 'GUARD_FAILED')
            self.assertEqual(launcher.channels, [])

    def test_identity_change_stops_before_commit_and_invalid_input_before_launch(self):
        launcher, _, host, result = self.run_case('changed_identity')
        self.assertEqual(result.status, 'UNKNOWN')
        self.assertEqual(launcher.commands, [1])
        before = len(launcher.channels)
        self.assertEqual(host.run('fixture').reason, 'REF_CONSUMED')
        self.assertEqual(len(launcher.channels), before)
        launcher = Launcher()
        result = Host(launcher).run('x' * 257)
        self.assertEqual(result.status, 'REFUSED')
        self.assertEqual(launcher.channels, [])


if __name__ == '__main__':
    unittest.main()
