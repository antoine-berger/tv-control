"""Only fake commands/processes and synthetic files; no SDK/ADB/emulator execution."""
import hashlib
import json
import tempfile
import time
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from transport_owned import COMPONENT, PACKAGE, OwnedLauncher, Pins, ReadyAvd, StageCleanupUnverified, TransportUnqualified, connectivity
from transport_wire import TransportFault
from transport_host import Outcome
from runtime_environment import verify_lab_build

SOURCE = 'a' * 40
BASE = '/data/app/~~synthetic/helper==/base.apk'


class FakePins:
    owner = Path('/owned/source/client-owner.sh')
    apk_hash, owner_hash = 'a' * 64, 'b' * 64
    def __init__(self):
        self.changed = False
    def guard(self, deadline):
        if self.changed:
            raise TransportFault('GUARD_FAILED')


class FakeAdb:
    adb = '/owned/sdk/platform-tools/adb'
    serial = 'emulator-5630'
    ports = SimpleNamespace(adb_port=49152, console_port=5630)
    env = {'PATH': '/owned/sdk/platform-tools', 'ADB_SERVER_SOCKET': 'tcp:127.0.0.1:49152'}
    def __init__(self):
        self.calls = []
        self.user, self.uid, self.base = 0, 10001, BASE
        self.shared = False
        self.mkdir_created = self.mkdir_error = False
        self.apk_hash, self.owner_hash = 'a' * 64, 'b' * 64
    def guard(self, deadline, require_device=False):
        if deadline <= time.monotonic():
            raise RuntimeError('synthetic expiry')
    def target(self, *words, **kwargs):
        self.calls.append((words, kwargs))
        if kwargs['deadline'] <= time.monotonic():
            raise RuntimeError('synthetic expiry')
        if words[0] == 'push':
            return 'synthetic push complete'
        words = words[2:]
        if words == ('am', 'get-current-user'):
            return str(self.user) + '\n'
        if words[:4] == ('cmd', 'package', 'list', 'packages'):
            return f'package:android uid:1000\npackage:{PACKAGE} uid:{self.uid}\n' + (
                f'package:org.synthetic.shared uid:{self.uid}\n' if self.shared else '')
        if words[:2] == ('pm', 'path'):
            return 'package:' + self.base + '\n'
        if words[:2] == ('/system/bin/toybox', 'sha256sum'):
            digest = self.apk_hash if words[2] == self.base else self.owner_hash
            return digest + '  ' + words[2] + '\n'
        if words[0] == 'getprop':
            return {'ro.kernel.qemu': '1\n', 'ro.build.version.sdk': '31\n', 'ro.product.cpu.abi': 'arm64-v8a\n'}[words[1]]
        if words[0] == 'mkdir':
            self.mkdir_created = True
            if self.mkdir_error:
                raise RuntimeError('synthetic postguard failure after remote mkdir')
            return ''
        if words[0] in ('rm', 'rmdir'):
            return ''
        raise AssertionError('unexpected fake command')


class Adapter(unittest.TestCase):
    def setUp(self):
        self.adb, self.pins = FakeAdb(), FakePins()
        self.ready = SimpleNamespace(adb=self.adb, abi='arm64-v8a', guard=lambda deadline: self.adb.guard(deadline, True))
        self.launcher = OwnedLauncher(self.ready, self.pins)
        self.deadline = time.monotonic() + 15

    def test_stages_only_owner_and_uses_installed_base_as_client_artifact(self):
        self.launcher.prepare(self.deadline)
        pushes = [words for words, _ in self.adb.calls if words[0] == 'push']
        self.assertEqual(len(pushes), 1)
        self.assertEqual(pushes[0][1], str(self.pins.owner))
        self.assertEqual(self.launcher.base, BASE)
        self.assertTrue(self.launcher.directory_created)
        self.assertTrue(all(call[1]['deadline'] == self.deadline for call in self.adb.calls))
        self.launcher.close_stage(self.deadline)
        self.assertFalse(self.launcher.directory_created)
        self.assertEqual([words[2] for words, _ in self.adb.calls if words[0] == 'shell'][-2:], ['rm', 'rmdir'])
        with self.assertRaises(TransportFault):
            self.launcher.prepare(self.deadline)

    def test_uncertain_mkdir_never_claims_cleanup_or_deletes_unproven_path(self):
        self.adb.mkdir_error = True
        with self.assertRaisesRegex(RuntimeError, 'postguard'):
            self.launcher.prepare(self.deadline)
        self.assertTrue(self.adb.mkdir_created)
        self.assertTrue(self.launcher.directory_attempted)
        self.assertFalse(self.launcher.directory_created)
        with self.assertRaises(StageCleanupUnverified):
            self.launcher.close_stage(self.deadline)
        self.assertFalse(any(words[2] in ('rm', 'rmdir') for words, _ in self.adb.calls if words[0] == 'shell'))
        self.assertFalse(any(words[0] == 'push' for words, _ in self.adb.calls))

    def test_stage_cleanup_failure_is_explicit_and_sticky(self):
        self.launcher.prepare(self.deadline)
        with patch.object(self.ready, 'guard', side_effect=RuntimeError('synthetic late identity failure')):
            with self.assertRaises(StageCleanupUnverified):
                self.launcher.close_stage(self.deadline)
        self.assertTrue(self.launcher.directory_attempted)
        self.assertTrue(self.launcher.directory_created)

    def test_fresh_user_uid_path_hash_and_local_owner_changes_refuse_before_spawn(self):
        for changed in ('user', 'uid', 'base', 'apk_hash', 'owner_hash', 'shared', 'local_owner'):
            with self.subTest(changed=changed):
                self.setUp()
                self.launcher.prepare(self.deadline)
                if changed == 'local_owner':
                    self.pins.changed = True
                else:
                    values = {'user': 1, 'uid': 10002, 'base': '/data/app/changed/base.apk',
                              'apk_hash': 'c' * 64, 'owner_hash': 'c' * 64, 'shared': True}
                    setattr(self.adb, changed, values[changed])
                with patch('transport_owned.subprocess.Popen') as spawn:
                    with self.assertRaises(TransportFault):
                        self.launcher.client('d' * 32)
                    spawn.assert_not_called()

    def test_spawn_has_fixed_private_adb_words_and_registers_before_pipe_setup(self):
        self.launcher.prepare(self.deadline)
        process = object()
        observed = []
        class Child:
            @staticmethod
            def require_supported():
                observed.append('supported')
            def __init__(self, owned, register):
                register(self)
                observed.append('registered')
                if owned is not process:
                    raise AssertionError('wrong owned child')
        with patch('transport_owned.subprocess.Popen', return_value=process) as spawn, patch('transport_owned.PipedChild', Child):
            self.launcher.instrumentation('d' * 32)
            argv = spawn.call_args.args[0]
            self.assertEqual(argv[1:9], ['-H', '127.0.0.1', '-P', '49152', '-s', 'emulator-5630', 'shell', '-T'])
            self.assertEqual(argv[9:], ['am', 'instrument', '-w', '-e', 'lab_instance', 'd' * 32, COMPONENT])
            self.launcher.client('d' * 32)
            self.assertEqual(spawn.call_args.args[0][9:], ['/system/bin/sh', self.launcher.owner_path, 'd' * 32, BASE, '10001'])
            self.assertTrue(spawn.call_args.kwargs['start_new_session'])
            self.assertNotIn('-n', spawn.call_args.args[0])
        self.assertEqual(observed, ['supported', 'registered', 'supported', 'registered'])
        self.assertEqual(len(self.launcher.owned()), 2)

    def test_expiry_and_remote_path_injection_never_spawn_or_stage(self):
        self.deadline = time.monotonic() - 1
        with self.assertRaises(RuntimeError):
            self.launcher.prepare(self.deadline)
        self.assertFalse(self.adb.calls)
        for path in (BASE + '\npackage:/other/base.apk', '/data/app/x/../y/base.apk', '/data/app/a;id/base.apk'):
            self.setUp()
            self.adb.base = path
            with self.assertRaises(TransportFault):
                self.launcher.prepare(self.deadline)
            self.assertFalse(any(words[0] == 'push' for words, _ in self.adb.calls))

    def test_ready_context_rejects_wrong_listener_or_exited_emulator(self):
        ready = ReadyAvd(self.adb, SimpleNamespace(pid=777), 'arm64-v8a')
        with patch('transport_owned.leader_exited', return_value=False), patch('transport_owned.owns_listener', return_value=False):
            with self.assertRaises(TransportFault):
                ready.guard(self.deadline)
        with patch('transport_owned.leader_exited', return_value=True), patch('transport_owned.owns_listener') as listener:
            with self.assertRaises(TransportFault):
                ready.guard(self.deadline)
            listener.assert_not_called()


class ConnectivityLifecycle(unittest.TestCase):
    def test_cleanup_after_budget_never_returns_a_qualified_result(self):
        clock = SimpleNamespace(now=100)
        class Launcher:
            def __init__(self, ready, pins):
                pass
            def prepare(self, deadline):
                clock.now = 101
            def close_stage(self, deadline):
                clock.now = 120
        class Probe:
            def __init__(self, launcher, deadline):
                self.deadline = deadline
            def probe_connectivity(self):
                return Outcome('CONNECTED', 'CANCELLED', 'VERIFIED', 'REAPED', True, True, {'server_ready': True})
        with patch('transport_owned.OwnedLauncher', Launcher), patch('transport_owned.Host', Probe), \
             patch('transport_owned.time.monotonic', side_effect=lambda: clock.now):
            with self.assertRaises(TransportUnqualified) as failure:
                connectivity(object(), object(), 130)
            self.assertEqual(failure.exception.transport_outcome['status'], 'UNKNOWN')
            self.assertEqual(failure.exception.transport_outcome['cleanup'], 'UNVERIFIED')
            self.assertEqual(failure.exception.transport_outcome['diagnostics'], {'server_ready': True})
            self.assertTrue(failure.exception.stage_cleanup_verified)

    def test_stage_failure_keeps_existing_transport_evidence_separate(self):
        class Launcher:
            def __init__(self, ready, pins):
                pass
            def prepare(self, deadline):
                pass
            def close_stage(self, deadline):
                raise StageCleanupUnverified()
        class Probe:
            def __init__(self, launcher, deadline):
                pass
            def probe_connectivity(self):
                return Outcome('CONNECTED', 'CANCELLED', 'VERIFIED', 'REAPED', True, True)
        with patch('transport_owned.OwnedLauncher', Launcher), patch('transport_owned.Host', Probe):
            with self.assertRaises(StageCleanupUnverified) as failure:
                connectivity(object(), object(), time.monotonic() + 15)
        self.assertEqual(failure.exception.transport_outcome['status'], 'CONNECTED')
        self.assertFalse(failure.exception.stage_cleanup_verified)

    def test_failed_preparation_and_probe_always_attempt_owned_stage_cleanup(self):
        for failed in ('prepare', 'probe'):
            events = []
            class Launcher:
                def __init__(self, ready, pins):
                    pass
                def prepare(self, deadline):
                    events.append('prepare')
                    if failed == 'prepare':
                        raise TransportFault('GUARD_FAILED')
                def close_stage(self, deadline):
                    events.append('close')
            class Probe:
                def __init__(self, launcher, deadline):
                    pass
                def probe_connectivity(self):
                    events.append('probe')
                    return Outcome('UNKNOWN', 'CLEANUP_UNVERIFIED', 'UNVERIFIED', 'UNVERIFIED', True, False)
            with patch('transport_owned.OwnedLauncher', Launcher), patch('transport_owned.Host', Probe):
                with self.assertRaises(TransportFault):
                    connectivity(object(), object(), time.monotonic() + 15)
            self.assertEqual(events[-1], 'close')
            self.assertEqual(events.count('close'), 1)


class BuildPins(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.build = self.root / 'build'
        self.build.mkdir()
        self.apk, self.owner, self.provenance = self.build / 'lab-helper.apk', self.root / 'owner.sh', self.root / 'provenance.json'
        self.apk.write_bytes(b'synthetic signed APK fixture')
        self.owner.write_bytes(b'synthetic pinned owner fixture')
        self.provenance.write_text('{}')
        digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
        self.receipt = dict(schema=1, lab_apk_sha256=digest(self.apk), lab_certificate_sha256='c' * 64,
                            owner_sha256=digest(self.owner), lab_package=PACKAGE, instrumentation_component=COMPONENT,
                            source_commit=SOURCE, sdk_provenance_sha256=digest(self.provenance))
        self.save()
    def save(self):
        (self.build / 'lab-build-receipt.json').write_text(json.dumps(self.receipt))
    def load(self):
        return Pins.load(self.build, self.owner, self.provenance, SOURCE, time.monotonic() + 5)
    def test_exact_local_receipt_loads_without_any_device_or_tool(self):
        pins = self.load()
        self.assertEqual(pins.apk, self.apk)
        self.assertEqual(pins.certificate_hash, 'c' * 64)
        pins.guard(time.monotonic() + 5)
    def test_unknown_stale_source_or_bad_identity_fields_are_rejected(self):
        for key, value in [('extra', True), ('source_commit', 'f' * 40), ('lab_package', 'org.other'),
                           ('instrumentation_component', 'org.other/Other'), ('lab_certificate_sha256', 'bad'), ('schema', True)]:
            with self.subTest(key=key):
                original = self.receipt.copy()
                self.receipt[key] = value
                self.save()
                with self.assertRaises(RuntimeError):
                    self.load()
                self.receipt = original
        self.save()
        self.apk.write_bytes(b'changed after build')
        with self.assertRaises(RuntimeError):
            self.load()


if __name__ == '__main__':
    unittest.main()
