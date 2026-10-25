#!/usr/bin/env python3
"""Signal shutdown of directly owned CLI bridges and loopback-only fake ADB."""
import json
import os
import pathlib
import select
import signal
import subprocess
import sys
import time
import unittest

from journey_support.fixture import JourneyFixture

BINARY = pathlib.Path(sys.argv.pop(1)).resolve() if len(sys.argv) > 1 else pathlib.Path('target/debug/tv-control').resolve()


class PipeRpc:
    def __init__(self, child):
        self.child = child
        self.pending = b''
        self.next_id = 1

    def send(self, method, params, identity=None):
        frame = {'jsonrpc': '2.0', 'method': method, 'params': params}
        if identity is not None:
            frame['id'] = identity
        self.child.stdin.write(json.dumps(frame).encode() + b'\n')
        self.child.stdin.flush()

    def call(self, method, params):
        identity = self.next_id
        self.next_id += 1
        self.send(method, params, identity)
        deadline = time.monotonic() + 5
        while True:
            while b'\n' not in self.pending:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not select.select([self.child.stdout], [], [], remaining)[0]:
                    raise AssertionError('fixture MCP response timeout')
                data = os.read(self.child.stdout.fileno(), 4096)
                if not data:
                    raise AssertionError('fixture MCP closed before response')
                self.pending += data
                if len(self.pending) > 65_536:
                    raise AssertionError('fixture MCP output bound')
            line, self.pending = self.pending.split(b'\n', 1)
            reply = json.loads(line)
            if reply.get('id') == identity:
                if 'error' in reply:
                    raise AssertionError(reply)
                return reply['result']

    def initialize(self):
        self.call('initialize', {'protocolVersion': '2025-11-25', 'capabilities': {},
                                'clientInfo': {'name': 'bridge-signal-fixture', 'version': '1'}})
        self.send('notifications/initialized', {})


class BridgeSignals(JourneyFixture):
    binary = BINARY

    def setUp(self):
        super().setUp()
        self.owned = []
        state = self.connection_parent / 'connection'
        state.mkdir(mode=0o700)
        intent = {'schema': 1, 'installation_id': 'signal-fixture', 'revision': 'signal-revision',
                  'device': 'screen', 'configuration': self.config.read_text(),
                  'controller_binary': str(self.binary)}
        path = state / 'runtime-intent.json'
        path.write_text(json.dumps(intent))
        path.chmod(0o600)

    def tearDown(self):
        try:
            for child in self.owned:
                if child.poll() is None:
                    child.kill()  # Only a fresh directly owned Popen child.
                child.wait(timeout=3)
                for pipe in [child.stdin, child.stdout, child.stderr]:
                    if pipe is not None:
                        pipe.close()
            deadline = time.monotonic() + 1
            while not self.port_free() and time.monotonic() < deadline:
                time.sleep(.01)
            free = self.port_free()
            # Release a leaked *fake* listener without hiding the original leak.
            (self.root / 'stop-adb-fixture').touch(mode=0o600)
            deadline = time.monotonic() + 3
            while not self.port_free() and time.monotonic() < deadline:
                time.sleep(.01)
            if not free and (self.root / 'mutation-entered').exists():
                # The old unhandled-HUP regression can also orphan the blocked
                # fake command. Its private marker path must acknowledge cleanup.
                finished = self.root / 'mutation-finished'
                while not finished.exists() and time.monotonic() < deadline:
                    time.sleep(.01)
                self.assertTrue(finished.exists(), 'orphaned fake command did not stop')
            self.assertTrue(free, 'bridge signal leaked its owned fake ADB listener')
        finally:
            super().tearDown()

    def start_bridge(self):
        state = self.connection_parent / 'connection'
        generation = state / 'bridge-generation.json'
        previous = generation.read_bytes() if generation.exists() else None
        child = subprocess.Popen([str(self.binary), 'bridge', str(self.config), str(state)],
                                 env=self.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, start_new_session=True)
        self.owned.append(child)
        deadline = time.monotonic() + 8
        while not generation.exists() or generation.read_bytes() == previous:
            if child.poll() is not None:
                raise AssertionError('fixture bridge exited during startup')
            if time.monotonic() >= deadline:
                raise AssertionError('fixture bridge generation timeout')
            time.sleep(.01)
        self.assertFalse(self.port_free())
        rpc = PipeRpc(child)
        rpc.initialize()
        return child, rpc

    def stop_with(self, child, sig):
        self.assertIsNone(child.poll())
        child.send_signal(sig)
        # Keep stdin open until exit: EOF must not provide a competing shutdown.
        self.assertEqual(child.wait(timeout=5), 0)
        self.assertTrue(self.port_free(), 'signal must release the owned ADB listener')

    def test_sigint_cleans_owned_bridge(self):
        child, _ = self.start_bridge()
        self.stop_with(child, signal.SIGINT)

    def test_sigterm_cleans_owned_bridge(self):
        child, _ = self.start_bridge()
        self.stop_with(child, signal.SIGTERM)

    def test_sighup_cleans_owned_bridge_without_stdin_eof(self):
        child, _ = self.start_bridge()
        self.stop_with(child, signal.SIGHUP)

    def test_hangup_after_possible_dispatch_persists_unknown_without_replay(self):
        # Extend only this private executable, never a real ADB program. Record
        # the mutation boundary, then block until the owned subprocess is cancelled.
        adb = self.bin / 'adb'
        original = adb.read_text()
        branch = '''elif len(args)>1 and args[-2]=='shell' and shlex.split(args[-1])==['sh','-c','exec input "$@" 2>&1','tv-control-input','keyevent','3']:
    import signal,time
    signal.signal(signal.SIGTERM,lambda *_:sys.exit(0))
    with (root/'navigation-dispatches').open('a') as output:output.write('1\\n')
    (root/'mutation-entered').touch()
    try:
        while not (root/'release-mutation').exists() and not (root/'stop-adb-fixture').exists():time.sleep(.01)
    finally:(root/'mutation-finished').touch()
'''
        self.assertIn('else:sys.exit(91)', original)
        adb.write_text(original.replace('else:sys.exit(91)', branch + 'else:sys.exit(91)'))
        child, rpc = self.start_bridge()
        request = {'request_id': 'interrupted-navigation', 'device': 'screen',
                   'action': {'type': 'navigate', 'keys': ['home']},
                   'expires_at_unix_ms': int(time.time() * 1000) + 10_000}
        rpc.send('tools/call', {'name': 'tv_control', 'arguments': request}, 100)
        deadline = time.monotonic() + 5
        while not (self.root / 'mutation-entered').exists():
            self.assertIsNone(child.poll())
            if time.monotonic() >= deadline:
                raise AssertionError('fixture mutation boundary not reached')
            time.sleep(.01)
        self.stop_with(child, signal.SIGHUP)
        journal = self.connection_parent / '.tv-control-operations/operations.json'
        entry = json.loads(journal.read_text())['entries'][request['request_id']]
        self.assertEqual(entry['reply']['status'], 'outcome_unknown')
        self.assertEqual(entry['reply']['code'], 'cancelled')
        self.assertIsNotNone(entry['completed_at'])
        count = (self.root / 'navigation-dispatches').read_bytes()
        self.assertEqual(count, b'1\n')
        # A restarted bridge must decode and retain the result, not send it again.
        restarted, rpc = self.start_bridge()
        before = (self.root / 'adb-events.jsonl').read_bytes()
        result = rpc.call('tools/call', {'name': 'tv_operation',
                                      'arguments': {'request_id': request['request_id']}})
        self.assertEqual(result['structuredContent']['status'], 'outcome_unknown')
        request['expires_at_unix_ms'] = int(time.time() * 1000) + 10_000
        result = rpc.call('tools/call', {'name': 'tv_control', 'arguments': request})
        self.assertEqual(result['structuredContent']['status'], 'outcome_unknown')
        self.assertTrue(result['structuredContent']['cached'])
        self.assertEqual((self.root / 'navigation-dispatches').read_bytes(), count)
        self.assertEqual((self.root / 'adb-events.jsonl').read_bytes(), before)
        self.stop_with(restarted, signal.SIGTERM)


if __name__ == '__main__':
    unittest.main()
