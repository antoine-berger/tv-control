#!/usr/bin/env python3
"""Explicit recovery of existing controllers using only private loopback fixtures."""
import json
import pathlib
import socket
import subprocess
import sys
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from journey_support.fixture import JourneyFixture, SECRET

BINARY = pathlib.Path(sys.argv.pop(1)).resolve() if len(sys.argv) > 1 else pathlib.Path('target/debug/tv-control').resolve()


class ControllerRecovery(JourneyFixture):
    binary = BINARY

    def prepare_existing(self):
        self.mcp_health_fixture()
        first = self.terminal()
        self.approve(first, persist=True)
        first.line('q')
        self.assertNotEqual(first.wait(), 0)
        state = self.connection_parent / 'connection'
        identity = state / 'adb/adb-home/identity-sentinel'
        identity.write_bytes(b'fixture-existing-adb-identity')
        identity.chmod(0o600)
        journal = self.connection_parent / '.tv-control-operations/operations.json'
        journal.parent.mkdir(mode=0o700, exist_ok=True)
        resource = json.loads(self.config.read_text())['devices']['screen']['coordination_group']
        journal.write_text(json.dumps({'schema': 1, 'entries': {'interrupted-fixture': {
            'device': 'screen', 'intent': {'type': 'pause'}, 'resource': resource,
            'cancelled': False, 'completed_at': 0,
            'reply': {'request_id': 'interrupted-fixture', 'status': 'outcome_unknown',
                      'code': 'operation_interrupted', 'observed': None, 'choices': [],
                      'cached': False, 'local_elapsed_ms': 0},
        }}}))
        journal.chmod(0o600)
        snapshot = {path: path.read_bytes() for path in [
            self.config, state / 'runtime-intent.json',
            state / 'runtime-credential.json', identity, journal,
        ]}
        return self.runtime_state(), snapshot

    def assert_preserved(self, snapshot):
        adb_before = (self.root / 'adb-events.jsonl').read_bytes()
        channel = self.channel()
        recovered = channel.call('tools/call', {'name': 'tv_operation', 'arguments': {'request_id': 'interrupted-fixture'}})
        result = recovered['result']['structuredContent']
        self.assertEqual(result['status'], 'outcome_unknown', recovered)
        self.assertEqual(result['code'], 'operation_interrupted', recovered)
        replay = channel.call('tools/call', {'name': 'tv_control', 'arguments': {
            'request_id': 'interrupted-fixture', 'device': 'screen', 'action': {'type': 'pause'},
            'expires_at_unix_ms': int(time.time() * 1000) + 5000,
        }})
        result = replay['result']['structuredContent']
        self.assertEqual(result['status'], 'outcome_unknown', replay)
        self.assertEqual(result['code'], 'operation_interrupted', replay)
        self.assertTrue(result['cached'], replay)
        self.assertEqual((self.root / 'adb-events.jsonl').read_bytes(), adb_before)
        for path, expected in snapshot.items():
            self.assertEqual(path.read_bytes(), expected, path.name)
        self.assert_private_records(allow_saved=True)

    def tunnel_mutations(self):
        return [event['args'] for event in map(json.loads, (self.root / 'tunnel-events.jsonl').read_text().splitlines())
                if event['args'][:2] in [['runtimes', 'connect'], ['runtimes', 'stop']]]

    def missing_endpoint(self):
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
        (self.root / 'mcp-health-url').write_text(f'http://127.0.0.1:{port}/health/mcp')

    def test_unavailable_health_decline_preserves_process_identity_and_journal(self):
        old, snapshot = self.prepare_existing()
        self.missing_endpoint()
        before = self.tunnel_mutations()
        adb_before = (self.root / 'adb-events.jsonl').read_bytes()
        generation = (self.connection_parent / 'connection/bridge-generation.json').read_bytes()
        second = self.terminal()
        second.until('Resume this saved controller')
        # Reaching the decision must perform no stop, start, ADB or proof reset.
        self.assertEqual(self.tunnel_mutations(), before)
        self.assertEqual((self.root / 'adb-events.jsonl').read_bytes(), adb_before)
        self.assertEqual(self.runtime_state()['bridge_pid'], old['bridge_pid'])
        second.line('n')
        self.assertNotEqual(second.wait(), 0)
        self.assertEqual(self.tunnel_mutations(), before)
        self.assertEqual((self.root / 'adb-events.jsonl').read_bytes(), adb_before)
        self.assertEqual((self.connection_parent / 'connection/bridge-generation.json').read_bytes(), generation)
        self.assert_preserved(snapshot)

    def test_http_failure_explicit_restart_preserves_existing_access(self):
        old, snapshot = self.prepare_existing()
        mode = self.root / 'recovery-health-mode'
        mode.write_text('429')

        class Health(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                if self.path != '/health/mcp':
                    self.send_error(404)
                    return
                if mode.read_text() == '429':
                    self.send_response(429)
                    self.send_header('Content-Length', '0')
                    self.end_headers()
                    return
                body = b'{"schema_version":1,"component":"mcp","status":"unknown","state":"not_observed"}'
                self.send_response(200)
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        server = ThreadingHTTPServer(('127.0.0.1', 0), Health)
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()

        def cleanup():
            server.shutdown()
            server.server_close()
            worker.join(timeout=2)
            self.assertFalse(worker.is_alive(), 'owned health fixture did not stop')

        self.addCleanup(cleanup)
        (self.root / 'mcp-health-url').write_text(f'http://127.0.0.1:{server.server_port}/health/mcp')
        before = self.tunnel_mutations()
        adb_before = (self.root / 'adb-events.jsonl').read_bytes()
        second = self.terminal()
        second.until('Resume this saved controller')
        self.assertEqual(self.tunnel_mutations(), before)
        self.assertEqual((self.root / 'adb-events.jsonl').read_bytes(), adb_before)
        mode.write_text('ok')
        second.line('y')
        self.decline_verification(second)
        self.assertEqual(second.wait(), 0, second.text)
        self.assert_maintenance_no_wizard(second)
        current = self.runtime_state()
        self.assertNotEqual(current['bridge_pid'], old['bridge_pid'])
        self.assertEqual(current['alias'], old['alias'])
        self.assertEqual(current['tunnel_id'], old['tunnel_id'])
        self.assertEqual(len(self.tunnel_mutations()), len(before) + 2)
        self.assert_preserved(snapshot)
        self.assertNotIn(SECRET, second.text)

    def test_stopped_controller_resumes_without_new_access_or_journal_reset(self):
        old, snapshot = self.prepare_existing()
        result = subprocess.run([str(self.binary), 'disconnect', str(self.config), '--json'],
                                env=self.env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(self.port_free())
        before = self.tunnel_mutations()
        adb_before = (self.root / 'adb-events.jsonl').read_bytes()
        second = self.terminal()
        second.until('Resume this saved controller')
        self.assertEqual(self.tunnel_mutations(), before)
        self.assertEqual((self.root / 'adb-events.jsonl').read_bytes(), adb_before)
        second.line('y')
        self.decline_verification(second)
        self.assertEqual(second.wait(), 0, second.text)
        self.assert_maintenance_no_wizard(second)
        self.assertEqual(self.runtime_state()['alias'], old['alias'])
        self.assertEqual(self.runtime_state()['tunnel_id'], old['tunnel_id'])
        self.assertFalse(self.port_free())
        self.assertEqual(len(self.tunnel_mutations()), len(before) + 1)
        self.assert_preserved(snapshot)


if __name__ == '__main__':
    unittest.main()
