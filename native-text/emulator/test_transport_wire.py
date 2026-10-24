import struct
import unittest
from transport_wire import (InputFrames, MAX_INPUT, PrivateAppend, PrivateRef,
                            Responses, TransportFault)

INSTANCE = '0123456789abcdef0123456789abcdef'


def frame(opcode, fields=b''):
    body = struct.pack('>IBB', 0x4E544C31, 1, opcode) + fields
    return struct.pack('>I', len(body)) + body


def reference(expiry=4000):
    return frame(0x81, b'e' * 16 + b'r' * 16 + struct.pack('>q', expiry))


def closed():
    return frame(0x82, b'\x01')


def owner(instance=INSTANCE, status=0):
    return frame(0x83, bytes.fromhex(instance) + bytes([status]))


class Wire(unittest.TestCase):
    def test_commands_match_frozen_java_layout(self):
        encoder = InputFrames()
        self.assertEqual(encoder.observe().hex(), '000000064e544c310101')
        self.assertEqual(encoder.cancel().hex(), '000000064e544c310103')
        request = encoder.commit(PrivateRef(b'e' * 16, b'r' * 16, 4000), PrivateAppend('a'))
        self.assertEqual(len(request), 47)
        self.assertEqual(request[:10].hex(), '0000002b4e544c310102')
        self.assertEqual(request[10:42], b'e' * 16 + b'r' * 16)
        self.assertEqual(request[42:], b'\x00\x00\x00\x01a')

    def test_utf16_limit_and_strict_utf8_without_exception_payload(self):
        PrivateAppend('x' * 256)
        PrivateAppend('\U0001f600' * 128)
        for text in ('', 'x' * 257, '\U0001f600' * 129, '\ud800', '\udfff'):
            with self.assertRaises(TransportFault) as error:
                PrivateAppend(text)
            self.assertIsNone(error.exception.__context__)
        value = PrivateAppend('private synthetic canary')
        self.assertEqual(repr(value), '<PrivateAppend>')
        self.assertEqual(repr(PrivateRef(b'e' * 16, b'r' * 16, 1)), '<PrivateRef>')

    def test_unbounded_or_overridden_text_is_rejected_before_encoding(self):
        class Sentinel(str):
            def encode(self, *args, **kwargs):
                raise AssertionError('private encode hook must not run')
        for text in [Sentinel('x' * 100000), Sentinel('small'), 'x' * 100000]:
            with self.assertRaises(TransportFault):
                PrivateAppend(text)

    def test_per_connection_frame_and_byte_budgets_are_not_reset(self):
        encoder = InputFrames()
        for _ in range(6):
            encoder.observe()
        with self.assertRaises(TransportFault):
            encoder.cancel()
        encoder = InputFrames()
        encoder._bytes = MAX_INPUT - 9
        with self.assertRaises(TransportFault):
            encoder.observe()

    def test_reference_is_copied_not_renewed_or_publicly_rendered(self):
        epoch, ref = bytearray(b'e' * 16), bytearray(b'r' * 16)
        value = PrivateRef(epoch, ref, 123)
        epoch[0] = ref[0] = 0
        result = InputFrames().commit(value, PrivateAppend('a'))
        self.assertEqual(result[10:42], b'e' * 16 + b'r' * 16)
        self.assertEqual(value.expires, 123)

    def test_fragmented_response_requires_both_close_and_remote_reap(self):
        parser = Responses(INSTANCE)
        wire = reference() + closed() + owner()
        self.assertEqual(len(wire), 88)
        for value in wire:
            parser.feed(bytes([value]))
        parser.eof()
        self.assertEqual(parser.reference.expires, 4000)
        self.assertTrue(parser.channel_closed)
        self.assertEqual(parser.remote_exit, 0)
        early = Responses(INSTANCE)
        early.feed(closed() + owner())
        early.eof()
        self.assertIsNone(early.reference)

    def test_channel_closed_does_not_prove_remote_client_exit(self):
        parser = Responses(INSTANCE)
        parser.feed(reference() + closed())
        with self.assertRaises(TransportFault):
            parser.eof()
        self.assertIsNone(parser.remote_exit)

    def test_malformed_or_stale_response_never_resynchronizes(self):
        cases = [
            owner() + closed(), reference() + reference(), closed() + closed(),
            closed() + owner('1' * 32), closed() + owner(status=2),
            frame(0x82, b'\x00'), frame(0x80), struct.pack('>I', 0xFFFFFFFF),
            b'raw diagnostic' + closed(), reference(0), reference(-1),
            reference().replace(b'NTL1', b'BAD1', 1),
            reference()[:8] + b'\x02' + reference()[9:],
            closed() + owner() + b'junk',
        ]
        for wire in cases:
            with self.subTest(size=len(wire)), self.assertRaises(TransportFault):
                parser = Responses(INSTANCE)
                parser.feed(wire)
                parser.eof()

    def test_truncation_and_oversized_output_are_fixed_faults(self):
        for wire in [b'', reference()[:3], reference()[:-1], closed() + owner()[:-1], b'x' * 89]:
            with self.assertRaises(TransportFault) as error:
                parser = Responses(INSTANCE)
                parser.feed(wire)
                parser.eof()
            self.assertNotIn('xxxx', str(error.exception))
            self.assertIsNone(error.exception.__context__)


if __name__ == '__main__':
    unittest.main()
