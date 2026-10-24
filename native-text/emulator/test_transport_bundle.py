import unittest
from transport_bundle import (CLEANUP, ENTERED, FINAL_KEYS, INSTANCE, PROTOCOL,
                              READY, REASON, TEXT, Instrumentation)
from transport_wire import TransportFault

PUBLIC_INSTANCE = '0123456789abcdef0123456789abcdef'


def ready(instance=PUBLIC_INSTANCE):
    fields = {PROTOCOL: '1', INSTANCE: instance, READY: 'READY'}
    return (''.join('INSTRUMENTATION_STATUS: ' + k + '=' + v + '\n'
                    for k, v in fields.items()) + 'INSTRUMENTATION_STATUS_CODE: 1\n').encode()


def final(**changes):
    fields = {PROTOCOL: '1', INSTANCE: PUBLIC_INSTANCE, TEXT: 'verified',
              CLEANUP: 'VERIFIED', ENTERED: 'YES', REASON: 'NONE'}
    fields.update(changes)
    return (''.join('INSTRUMENTATION_RESULT: ' + k + '=' + v + '\n'
                    for k, v in fields.items()) + 'INSTRUMENTATION_CODE: 0\n').encode()


class Bundle(unittest.TestCase):
    def test_ready_is_separate_progress_not_terminal(self):
        parser = Instrumentation(PUBLIC_INSTANCE)
        parser.feed(ready())
        self.assertTrue(parser.ready)
        self.assertIsNone(parser.final)
        with self.assertRaises(TransportFault):
            parser.eof()

    def test_final_is_correlated_unique_and_only_after_finish_code(self):
        parser = Instrumentation(PUBLIC_INSTANCE)
        message = ready() + final()
        for value in message:
            parser.feed(bytes([value]))
        parser.eof()
        self.assertEqual(parser.final.keys(), FINAL_KEYS)
        self.assertEqual(parser.final[TEXT], 'verified')
        unfinished = Instrumentation(PUBLIC_INSTANCE)
        unfinished.feed(final().replace(b'INSTRUMENTATION_CODE: 0\n', b''))
        with self.assertRaises(TransportFault):
            unfinished.eof()

    def test_remote_uncertain_cleanup_failure_is_not_promoted(self):
        parser = Instrumentation(PUBLIC_INSTANCE)
        parser.feed(ready() + final(**{TEXT: 'uncertain', CLEANUP: 'UNVERIFIED',
                                     REASON: 'CLEANUP_UNVERIFIED'}))
        parser.eof()
        self.assertEqual(parser.final[TEXT], 'uncertain')
        self.assertEqual(parser.final[CLEANUP], 'UNVERIFIED')

    def test_legacy_missing_instance_is_not_lab_transport(self):
        for message in [final(**{CLEANUP: 'NOT_APPLICABLE'}),
                        final().replace(('INSTRUMENTATION_RESULT: ' + INSTANCE + '=' + PUBLIC_INSTANCE + '\n').encode(), b'')]:
            with self.assertRaises(TransportFault):
                parser = Instrumentation(PUBLIC_INSTANCE)
                parser.feed(message)
                parser.eof()

    def test_unknown_duplicate_stale_or_wrong_groups_fail_closed(self):
        duplicate = ('INSTRUMENTATION_RESULT: ' + PROTOCOL + '=1\n').encode()
        cases = [ready('0' * 32), ready() + ready(),
                 duplicate + final(), final() + final(),
                 final(**{INSTANCE: 'f' * 32}), final(**{PROTOCOL: '2'}),
                 final(**{REASON: 'exception PRIVATE_CANARY'}),
                 final().replace(b'INSTRUMENTATION_RESULT:', b'INSTRUMENTATION_STATUS:'),
                 final().replace(b'INSTRUMENTATION_CODE: 0', b'INSTRUMENTATION_CODE: -1'),
                 ready().replace(b'INSTRUMENTATION_STATUS_CODE: 1', b'INSTRUMENTATION_STATUS_CODE: 0'),
                 b'raw PRIVATE_CANARY\n', b'\xff\n']
        for message in cases:
            with self.subTest(size=len(message)), self.assertRaises(TransportFault) as error:
                parser = Instrumentation(PUBLIC_INSTANCE)
                parser.feed(message)
                parser.eof()
            self.assertNotIn('PRIVATE_CANARY', str(error.exception))
            self.assertIsNone(error.exception.__context__)

    def test_verified_requires_real_entry_none_reason_and_verified_cleanup(self):
        for change in [{ENTERED: 'NO'}, {REASON: 'ENGINE_REFUSED'}, {CLEANUP: 'UNVERIFIED'}]:
            with self.assertRaises(TransportFault):
                parser = Instrumentation(PUBLIC_INSTANCE)
                parser.feed(ready() + final(**change))

    def test_byte_and_line_limits_precede_raw_output_retention(self):
        for message in [b'x' * 8193, b'x' * 513, b'INSTRUMENTATION_RESULT: unknown=SECRET\n']:
            with self.assertRaises(TransportFault) as error:
                Instrumentation(PUBLIC_INSTANCE).feed(message)
            self.assertNotIn('SECRET', str(error.exception))


if __name__ == '__main__':
    unittest.main()
