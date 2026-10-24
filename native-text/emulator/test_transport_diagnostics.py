"""Sanitized projections and fake host phases only; no SDK, socket, ADB or VM."""
import struct
import unittest
from transport_diagnostics import ClientFailure, FAILURE_REASONS, LIMIT, PHASES, PREFIX
from transport_host import Host
from transport_wire import MAGIC, Responses, TransportFault
from test_transport_bundle import PUBLIC_INSTANCE
from test_transport_host import Clock, Launcher
from test_transport_wire import closed, owner, reference


def failure_line(phase='CONNECT', reason='POLICY_DENIED', cleanup='UNVERIFIED'):
    return f'{PREFIX} phase={phase} reason={reason} cleanup={cleanup}\n'.encode('ascii')


def classify(chunks, eof=True):
    collector = ClientFailure()
    for chunk in chunks:
        collector.feed(chunk)
    if eof:
        collector.eof()
    return collector


class FailureLine(unittest.TestCase):
    def test_exact_one_line_is_only_valid_after_eof_for_every_fragmentation(self):
        line = failure_line()
        expected = dict(category='VALID', fields=dict(phase='CONNECT', reason='POLICY_DENIED', cleanup='UNVERIFIED'))
        for split in range(len(line) + 1):
            collector = classify([line[:split], line[split:]], eof=False)
            self.assertEqual(collector.public()['category'], 'TRUNCATED')
            collector.eof()
            self.assertEqual(collector.public(), expected)
            self.assertFalse(collector._pending)
        self.assertEqual(classify([bytes([byte]) for byte in line]).public(), expected)

    def test_all_closed_enums_fit_bound_and_none_is_forbidden(self):
        for phase in PHASES:
            for reason in FAILURE_REASONS:
                for cleanup in ('VERIFIED', 'UNVERIFIED'):
                    line = failure_line(phase, reason, cleanup)
                    self.assertLessEqual(len(line), LIMIT)
                    self.assertEqual(classify([line]).public()['category'], 'VALID')
        self.assertEqual(classify([failure_line(reason='NONE')]).public()['category'], 'UNKNOWN')

    def test_absent_truncated_unknown_duplicate_and_oversize_are_closed(self):
        cases = [([], 'ABSENT'), ([failure_line()[:-1]], 'TRUNCATED'),
                 ([b'PRIVATE_DIAGNOSTIC_CANARY\n'], 'UNKNOWN'),
                 ([failure_line().replace(b'CONNECT', b'PRIVATE_DIAGNOSTIC_CANARY')], 'UNKNOWN'),
                 ([failure_line().replace(b'\n', b'\r\n')], 'UNKNOWN'),
                 ([b'\xff\n'], 'UNKNOWN'), ([failure_line() + b'x\n'], 'DUPLICATE'),
                 ([b'x' * 159 + b'\n'], 'UNKNOWN'), ([b'x' * 160], 'TRUNCATED'),
                 ([b'x' * 161], 'OVERSIZE'), ([failure_line(), b'x' * 161], 'OVERSIZE')]
        for chunks, category in cases:
            with self.subTest(category=category):
                collector = classify(chunks)
                public = collector.public()
                self.assertEqual(public, dict(category=category, fields=None))
                self.assertFalse(collector._pending)
                self.assertNotIn('PRIVATE_DIAGNOSTIC_CANARY', repr(public) + repr(collector))

    def test_bound_is_checked_before_append_and_remains_sticky(self):
        class FullBuffer:
            def __len__(self):
                return LIMIT
            def extend(self, data):
                raise AssertionError('must reject before append')
            def clear(self):
                pass
        collector = ClientFailure()
        collector._pending = FullBuffer()
        collector.feed(b'x')
        collector.feed(failure_line())
        collector.eof()
        self.assertEqual(collector.public()['category'], 'OVERSIZE')
        collector = classify([b'x' * 1000000])
        self.assertFalse(collector._pending)

    def test_after_eof_cannot_reclassify_a_second_line_as_valid(self):
        collector = classify([failure_line()])
        collector.feed(failure_line())
        self.assertEqual(collector.public(), dict(category='DUPLICATE', fields=None))


class WireProjection(unittest.TestCase):
    def test_owner_without_closed_is_rejected_with_safe_classes(self):
        parser = Responses(PUBLIC_INSTANCE)
        with self.assertRaises(TransportFault) as failure:
            parser.feed(owner(status=1))
        self.assertEqual(failure.exception.args, ('INVALID_FRAME',))
        self.assertEqual(parser.diagnostics(), dict(byte_count=27, closed=False, owner=False,
                         length_class='BODY_23', opcode_class='OWNER', rejected_at='ORDER_OR_FIELDS'))

    def test_success_fragmentation_has_identical_projection_and_no_private_fields(self):
        data = reference() + closed() + owner()
        expected = dict(byte_count=88, closed=True, owner=True, length_class='BODY_23', opcode_class='OWNER', rejected_at=None)
        for split in range(len(data) + 1):
            parser = Responses(PUBLIC_INSTANCE)
            parser.feed(data[:split])
            parser.feed(data[split:])
            parser.eof()
            self.assertEqual(parser.diagnostics(), expected)
            self.assertNotIn(PUBLIC_INSTANCE, repr(parser.diagnostics()))

    def test_invalid_headers_fields_and_canaries_never_expose_raw_values(self):
        frame = lambda magic, version, opcode, fields: struct.pack('>IIBB', len(fields) + 6, magic, version, opcode) + fields
        cases = [(b'PRIVATE_WIRE_CANARY', 'LENGTH', 'INVALID', None),
                 (frame(0, 1, 0x82, b'\x01'), 'MAGIC', 'BODY_7', 'CLOSED'),
                 (frame(MAGIC, 2, 0x82, b'\x01'), 'VERSION', 'BODY_7', 'CLOSED'),
                 (frame(MAGIC, 1, 0xff, b'\x01'), 'ORDER_OR_FIELDS', 'BODY_7', 'OTHER'),
                 (closed() + owner(instance='f' * 32), 'ORDER_OR_FIELDS', 'BODY_23', 'OWNER')]
        for data, rejected, length, opcode in cases:
            for split in range(len(data) + 1):
                parser = Responses(PUBLIC_INSTANCE)
                fault = None
                for part in (data[:split], data[split:]):
                    if fault is not None:
                        parser.count_discarded(part)
                        continue
                    try:
                        parser.feed(part)
                    except TransportFault as error:
                        fault = error.args[0]
                self.assertEqual(fault, 'INVALID_FRAME')
                projection = parser.diagnostics()
                self.assertEqual((projection['rejected_at'], projection['length_class'], projection['opcode_class']),
                                 (rejected, length, opcode))
                self.assertEqual(projection['byte_count'], len(data))
                self.assertNotIn('PRIVATE_WIRE_CANARY', repr(projection))
                self.assertNotIn('f' * 32, repr(projection))
        parser = Responses(PUBLIC_INSTANCE)
        with self.assertRaises(TransportFault):
            parser.feed(b'PRIVATE_WIRE_CANARY' * 1000)
        parser.count_discarded(b'x' * 1000)
        self.assertEqual(parser.diagnostics()['byte_count'], 88)


class HostProjection(unittest.TestCase):
    def run_probe(self, mode='valid', stderr=None):
        launcher, clock = Launcher('valid' if mode == 'owner_only' else mode), Clock()
        original = launcher.command
        def command(frame):
            original(frame)
            if frame[9] == 3 and mode == 'owner_only':
                launcher.peer.output.clear()
                launcher.peer.output.append(owner(status=1))
            if frame[9] == 3 and stderr is not None:
                launcher.peer.errors.extend(stderr)
        launcher.command = command
        outcome = Host(launcher, clock=clock, wait=clock.wait,
                       entropy=lambda: PUBLIC_INSTANCE, budget=3).probe_connectivity()
        return outcome

    def test_owner_before_closed_preserves_clean_final_without_qualifying_client(self):
        result = self.run_probe(mode='owner_only')
        self.assertEqual(result.status, 'UNKNOWN')
        self.assertEqual(result.reason, 'INVALID_FRAME')
        self.assertFalse(result.qualified)
        diagnostics = result.public()['diagnostics']
        self.assertEqual(diagnostics['server_final'], dict(text='refused', cleanup='VERIFIED', entered='NO', reason='CANCELLED'))
        self.assertEqual(diagnostics['response'], dict(byte_count=27, closed=False, owner=False,
                         length_class='BODY_23', opcode_class='OWNER', rejected_at='ORDER_OR_FIELDS'))

    def test_valid_stderr_diagnostic_is_still_unknown_never_success(self):
        result = self.run_probe(stderr=[bytes([byte]) for byte in failure_line()])
        self.assertEqual(result.status, 'UNKNOWN')
        self.assertEqual(result.reason, 'STDERR_OUTPUT')
        self.assertFalse(result.qualified)
        diagnostic = result.public()['diagnostics']
        self.assertEqual(diagnostic['client_stderr']['category'], 'VALID')
        self.assertEqual(diagnostic['server_final'], dict(text='refused', cleanup='VERIFIED', entered='NO', reason='CANCELLED'))
        self.assertNotIn(PUBLIC_INSTANCE, repr(diagnostic))

    def test_unknown_stderr_has_no_raw_output_and_success_is_unchanged(self):
        result = self.run_probe(stderr=[b'PRIVATE_STDERR_CANARY\n'])
        self.assertEqual(result.reason, 'STDERR_OUTPUT')
        self.assertEqual(result.public()['diagnostics']['client_stderr']['category'], 'UNKNOWN')
        self.assertNotIn('PRIVATE_STDERR_CANARY', repr(result.public()))
        good = self.run_probe()
        self.assertEqual(good.status, 'CONNECTED')
        self.assertTrue(good.qualified)
        self.assertEqual(good.public()['diagnostics']['client_stderr']['category'], 'ABSENT')

    def test_server_tuple_is_absent_for_missing_bad_or_non_eof_final(self):
        for mode in ('stale_bundle', 'double_bundle', 'no_bundle', 'raw_output'):
            # These modes corrupt append results in the existing fake launcher.
            launcher, clock = Launcher(mode), Clock()
            result = Host(launcher, clock=clock, wait=clock.wait,
                          entropy=lambda: PUBLIC_INSTANCE, budget=.5).run('synthetic')
            self.assertIsNone(result.public()['diagnostics']['server_final'])
            self.assertEqual(result.status, 'UNKNOWN')


if __name__ == '__main__':
    unittest.main()
