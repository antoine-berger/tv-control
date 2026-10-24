"""Synthetic clocks and fake I/O only. Measurements never decide an outcome."""
import math
from pathlib import Path
from types import SimpleNamespace
import sys
import time
import unittest
from unittest.mock import patch

import owned_runtime
import transport_owned
from transport_host import Host
from transport_owned import OwnedLauncher, ReadyAvd
from test_transport_bundle import PUBLIC_INSTANCE
from test_transport_host import Clock, Launcher
from test_transport_owned import FakeAdb, FakePins, BASE


class HostTiming(unittest.TestCase):
    def probe(self, metrics=None, bad_clock=None):
        class MeasuredLauncher(Launcher):
            @property
            def client_timing(self):
                if isinstance(metrics, Exception):
                    raise metrics
                return metrics
            def guard(self, deadline):
                deadlines.append(deadline)
                super().guard(deadline)
        launcher, clock, deadlines = MeasuredLauncher(), Clock(), []
        def observed_clock():
            if sys._getframe(1).f_code.co_name == 'stamp' and bad_clock is not None:
                if isinstance(bad_clock, Exception):
                    raise bad_clock
                return bad_clock
            return clock.now
        result = Host(launcher, clock=observed_clock, wait=clock.wait,
                      entropy=lambda: PUBLIC_INSTANCE).probe_connectivity()
        return result, launcher, deadlines

    def test_metrics_valid_invalid_absent_or_throwing_never_change_calls_or_verdict(self):
        good = dict(guard_ms=0, popen_ms=0, incomplete=False)
        reference, launcher, deadlines = self.probe(good)
        self.assertTrue(reference.qualified)
        self.assertFalse(reference.diagnostics['timing']['incomplete'])
        for metrics in (None, {'guard_ms': True, 'popen_ms': 1, 'incomplete': False},
                        {'guard_ms': math.nan, 'popen_ms': 1, 'incomplete': False},
                        {'guard_ms': None, 'popen_ms': None, 'incomplete': False},
                        RuntimeError('PRIVATE_TIMING_CANARY')):
            with self.subTest(metrics_type=type(metrics).__name__):
                result, observed, observed_deadlines = self.probe(metrics)
                self.assertEqual((result.status, result.reason, result.qualified),
                                 (reference.status, reference.reason, reference.qualified))
                self.assertEqual(observed.commands, launcher.commands)
                self.assertEqual(observed.guards, launcher.guards)
                self.assertEqual(observed_deadlines, deadlines)
                self.assertTrue(result.diagnostics['timing']['incomplete'])
                self.assertNotIn('PRIVATE_TIMING_CANARY', repr(result.public()))

    def test_bad_observation_clocks_are_secondary_and_do_not_replace_verdict(self):
        good = dict(guard_ms=0, popen_ms=0, incomplete=False)
        for bad in (True, math.nan, math.inf, -math.inf, -1, RuntimeError('PRIVATE_TIMING_CANARY')):
            with self.subTest(kind=type(bad).__name__):
                result, launcher, deadlines = self.probe(good, bad)
                self.assertEqual(result.status, 'CONNECTED')
                self.assertTrue(result.qualified)
                self.assertEqual(launcher.commands, [3])
                self.assertEqual(set(deadlines), {15})
                self.assertTrue(result.diagnostics['timing']['incomplete'])
                self.assertTrue(all(value is None for value in result.diagnostics['timing']['host'].values()))
                self.assertNotIn('PRIVATE_TIMING_CANARY', repr(result.public()))

    def test_saturated_host_metrics_are_marked_incomplete_and_never_gate_success(self):
        result, launcher, deadlines = self.probe(dict(guard_ms=0, popen_ms=0, incomplete=False), 20)
        self.assertEqual(result.status, 'CONNECTED')
        self.assertEqual(launcher.commands, [3])
        self.assertEqual(set(deadlines), {15})
        self.assertEqual(set(result.diagnostics['timing']['host'].values()), {15000})
        self.assertTrue(result.diagnostics['timing']['incomplete'])

    def test_ready_recorded_once_and_distinct_guard_and_call_offsets(self):
        clock = Clock()
        class CostLauncher(Launcher):
            @property
            def client_timing(self):
                return dict(guard_ms=2890, popen_ms=0, incomplete=False)
            def guard(self, deadline):
                super().guard(deadline)
                if self.guards == 2:
                    clock.now += 2.89
            def client(self, instance):
                clock.now += 2.89
                return super().client(instance)
        result = Host(CostLauncher(), clock=clock, wait=clock.wait,
                      entropy=lambda: PUBLIC_INSTANCE).probe_connectivity()
        metrics = result.diagnostics['timing']
        self.assertEqual(metrics['host'], dict(ready_received_ms=0, host_client_guard_start_ms=0,
                         host_client_guard_end_ms=2890, client_call_start_ms=2890, client_call_end_ms=5780))
        self.assertFalse(metrics['incomplete'])
        # The fake server has no acceptance timeout; timing alone cannot reject it.
        self.assertEqual(result.status, 'CONNECTED')

    def test_failed_client_call_still_records_end_and_preserves_primary_failure(self):
        launcher, clock = Launcher('client_start_raises'), Clock()
        result = Host(launcher, clock=clock, wait=clock.wait,
                      entropy=lambda: PUBLIC_INSTANCE).probe_connectivity()
        self.assertEqual(result.reason, 'HOST_FAILURE')
        timing = result.diagnostics['timing']
        self.assertIsNotNone(timing['host']['client_call_end_ms'])
        self.assertTrue(timing['incomplete'])
        self.assertTrue(all(child.cleanups == 1 for child in launcher.channels))


class LauncherTiming(unittest.TestCase):
    def launcher(self, clock):
        adb = FakeAdb()
        ready = SimpleNamespace(adb=adb)
        launcher = OwnedLauncher(ready, FakePins(), clock=clock)
        launcher.deadline = time.monotonic() + 15
        launcher.owner_path, launcher.base, launcher.uid = '/synthetic/owner.sh', BASE, 10001
        return launcher

    def test_registration_precedes_post_popen_clock_and_property_returns_copy(self):
        clock = Clock()
        launcher = self.launcher(clock)
        events = []
        require_registered = [False]
        def measured_clock():
            if require_registered[0]:
                self.assertTrue(launcher.channels)
            return clock.now
        launcher._timing_clock = measured_clock
        def guard(deadline):
            events.append(('guard', deadline)); clock.now += 2.89
        def popen(*args, **kwargs):
            events.append(('popen', kwargs['start_new_session'])); clock.now += .12
            require_registered[0] = True
            return object()
        class Child:
            @staticmethod
            def require_supported():
                events.append(('support', True))
            def __init__(self, process, register):
                register(self)
                events.append(('adopted', True))
        with patch.object(launcher, 'guard', side_effect=guard), patch.object(transport_owned.subprocess, 'Popen', side_effect=popen), patch.object(transport_owned, 'PipedChild', Child):
            launcher.client(PUBLIC_INSTANCE)
        self.assertEqual([event[0] for event in events], ['guard', 'support', 'popen', 'adopted'])
        self.assertEqual(launcher.client_timing, dict(guard_ms=2890, popen_ms=120, incomplete=False))
        copy = launcher.client_timing
        copy['guard_ms'] = 15000
        self.assertEqual(launcher.client_timing['guard_ms'], 2890)

    def test_failure_end_metrics_preserve_exception_and_do_not_lose_owned_child(self):
        for fail_at in ('guard', 'popen', 'adopt'):
            clock = Clock()
            launcher = self.launcher(clock)
            primary = RuntimeError('primary fixture failure')
            def guard(deadline):
                clock.now += .1
                if fail_at == 'guard': raise primary
            def popen(*args, **kwargs):
                clock.now += .2
                if fail_at == 'popen': raise primary
                return object()
            class Child:
                @staticmethod
                def require_supported(): pass
                def __init__(self, process, register):
                    register(self)
                    raise primary
            with patch.object(launcher, 'guard', side_effect=guard), patch.object(transport_owned.subprocess, 'Popen', side_effect=popen), patch.object(transport_owned, 'PipedChild', Child):
                with self.assertRaises(RuntimeError) as failure:
                    launcher.client(PUBLIC_INSTANCE)
            self.assertIs(failure.exception, primary)
            self.assertEqual(launcher.client_timing['guard_ms'], 100)
            self.assertEqual(launcher.client_timing['popen_ms'], None if fail_at == 'guard' else 200)
            self.assertTrue(launcher.client_timing['incomplete'])
            self.assertEqual(len(launcher.channels), 1 if fail_at == 'adopt' else 0)

    def test_bad_or_saturated_timing_never_changes_spawn(self):
        for values in ((True, 1, 2, 3), (0, math.nan, 2, 3), (0, math.inf, 2, 3),
                       (2, 1, 3, 4), (0, 20, 21, 22)):
            iterator = iter(values)
            launcher = self.launcher(lambda: next(iterator))
            class Child:
                @staticmethod
                def require_supported(): pass
                def __init__(self, process, register): register(self)
            with patch.object(launcher, 'guard') as guard, patch.object(transport_owned.subprocess, 'Popen', return_value=object()) as spawn, patch.object(transport_owned, 'PipedChild', Child):
                launcher.client(PUBLIC_INSTANCE)
            guard.assert_called_once_with(launcher.deadline)
            spawn.assert_called_once()
            self.assertEqual(len(launcher.channels), 1)
            self.assertTrue(launcher.client_timing['incomplete'])
            for key in ('guard_ms', 'popen_ms'):
                value = launcher.client_timing[key]
                self.assertTrue(value is None or type(value) is int and 0 <= value <= 15000)


class SyntheticGuardCost(unittest.TestCase):
    def test_actual_guard_call_graph_can_exceed_accept_window_without_host_expiry(self):
        # Injected costs, not measured Android/host latencies or run7 attribution.
        clock = Clock()
        clock.now = 100
        counts = dict(adb_metadata=0, lsof=0, host_query=0)
        owner_path = '/data/local/tmp/synthetic-owner/client-owner.sh'
        def listener(*args, **kwargs):
            counts['lsof'] += 1; clock.now += .05
            return True
        def query(port, name, **kwargs):
            counts['host_query'] += 1; clock.now += .01
            return '0029' if name == 'host:version' else 'emulator-5630\tdevice\n'
        def command(argv, env, timeout):
            counts['adb_metadata'] += 1; clock.now += .2
            words = tuple(argv[argv.index('shell') + 2:])
            if words == ('am', 'get-current-user'): return '0\n'
            if words[:4] == ('cmd', 'package', 'list', 'packages'): return 'package:org.tvcontrol.nativehelper uid:10001\n'
            if words[:2] == ('pm', 'path'): return 'package:' + BASE + '\n'
            if words[0] == 'getprop':
                return {'ro.kernel.qemu': '1\n', 'ro.build.version.sdk': '31\n', 'ro.product.cpu.abi': 'arm64-v8a\n'}[words[1]]
            if words[:2] == ('/system/bin/toybox', 'sha256sum'):
                return ('a' * 64 if words[2] == BASE else 'b' * 64) + '  ' + words[2] + '\n'
            raise AssertionError('unexpected fake metadata call')
        adb = owned_runtime.PrivateAdb(Path('/synthetic/adb'), SimpleNamespace(adb_port=49152, console_port=5630), {})
        adb.process = SimpleNamespace(pid=123)
        adb.log, adb.protocol = SimpleNamespace(fileno=lambda: 0), 41
        launcher = OwnedLauncher(ReadyAvd(adb, SimpleNamespace(pid=456), 'arm64-v8a'), FakePins(), clock=clock)
        launcher.user, launcher.uid, launcher.base, launcher.owner_path = 0, 10001, BASE, owner_path
        with patch.object(owned_runtime.time, 'monotonic', clock), \
             patch.object(owned_runtime, 'leader_exited', return_value=False), patch.object(transport_owned, 'leader_exited', return_value=False), \
             patch.object(owned_runtime.os, 'fstat', return_value=SimpleNamespace(st_size=0)), \
             patch.object(owned_runtime, 'owns_listener', side_effect=listener), patch.object(transport_owned, 'owns_listener', side_effect=listener), \
             patch.object(owned_runtime, 'host_query', side_effect=query), patch.object(owned_runtime, 'command', side_effect=command):
            launcher.guard(115)
            first = clock.now - 100
            launcher.guard(115)
            total = clock.now - 100
        self.assertEqual(counts, dict(adb_metadata=16, lsof=38, host_query=68))
        self.assertEqual(round(first * 1000), 2890)
        self.assertEqual(round(total * 1000), 5780)
        self.assertGreater(total, 5)
        self.assertLess(total, 15)
