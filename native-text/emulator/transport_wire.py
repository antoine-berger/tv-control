"""Private, bounded lab frames. No Android calls, payload logging or public CLI."""
import struct

MAGIC = 0x4E544C31
MAX_FRAME = 16384
MAX_INPUT = 32768
MAX_FRAMES = 6
MAX_RESPONSES = 88  # One REF(50), CHANNEL_CLOSED(11), remote-owner proof(27).
REASONS = frozenset('NONE PEER_UNKNOWN PEER_MISMATCH POLICY_DENIED IDENTITY_CHANGED INVALID_FRAME LIMIT_EXCEEDED EDITOR_UNSUPPORTED REF_UNKNOWN REF_CONSUMED REF_EXPIRED CANCELLED DEADLINE ENGINE_REFUSED ENGINE_UNCERTAIN TRANSPORT_LOST CLEANUP_UNVERIFIED'.split())
HOST_REASONS = frozenset('HOST_FAILURE HOST_CANCELLED HOST_DEADLINE INVALID_BUNDLE CHANNEL_LOST CLIENT_EXIT_NONZERO LOCAL_CLEANUP_FAILED GUARD_FAILED STDERR_OUTPUT'.split())


class TransportFault(Exception):
    def __init__(self, code):
        super().__init__(code if code in REASONS | HOST_REASONS else 'HOST_FAILURE')


class PrivateAppend:
    def __init__(self, text):
        # Bound before encoding/allocation, and do not execute caller-defined
        # str-subclass encode/length hooks with private text.
        if type(text) is not str:
            raise TransportFault('INVALID_FRAME')
        if not text or len(text) > 256:
            raise TransportFault('LIMIT_EXCEEDED')
        if any(0xD800 <= ord(value) <= 0xDFFF for value in text):
            raise TransportFault('INVALID_FRAME')
        units = sum(2 if ord(value) > 0xFFFF else 1 for value in text)
        if units > 256:
            raise TransportFault('LIMIT_EXCEEDED')
        encoded = text.encode('utf-8', errors='strict')
        if len(encoded) > 2048:
            raise TransportFault('LIMIT_EXCEEDED')
        self._bytes = encoded

    def __repr__(self):
        return '<PrivateAppend>'


class PrivateRef:
    def __init__(self, epoch, reference, expires):
        if len(epoch) != 16 or len(reference) != 16 or not 0 < expires < 2**63:
            raise TransportFault('INVALID_FRAME')
        self._epoch, self._reference, self.expires = bytes(epoch), bytes(reference), expires

    def __repr__(self):
        return '<PrivateRef>'


class InputFrames:
    def __init__(self):
        self._bytes = self._frames = 0

    def _encode(self, opcode, fields=b''):
        body = struct.pack('>IBB', MAGIC, 1, opcode) + fields
        frame = struct.pack('>I', len(body)) + body
        if len(frame) > MAX_FRAME or self._bytes + len(frame) > MAX_INPUT or self._frames >= MAX_FRAMES:
            raise TransportFault('LIMIT_EXCEEDED')
        self._bytes += len(frame)
        self._frames += 1
        return frame

    def observe(self):
        return self._encode(1)

    def commit(self, reference, append):
        if not isinstance(reference, PrivateRef) or not isinstance(append, PrivateAppend):
            raise TransportFault('INVALID_FRAME')
        return self._encode(2, reference._epoch + reference._reference
                            + struct.pack('>I', len(append._bytes)) + append._bytes)

    def cancel(self):
        return self._encode(3)


class Responses:
    """Incremental private decoder; never scan arbitrary bytes for a magic marker."""
    def __init__(self, instance):
        if len(instance) != 32 or any(c not in '0123456789abcdef' for c in instance):
            raise TransportFault('INVALID_FRAME')
        self._instance = bytes.fromhex(instance)
        self._buffer = bytearray()
        self._total = 0
        self.reference = None
        self.channel_closed = False
        self.remote_exit = None
        self._diagnostic_count = 0
        self._length_class = self._opcode_class = self._rejected_at = None

    def count_discarded(self, chunk):
        # Counts saturate; even oversized/unrecognized output exposes no raw
        # prefix, arbitrary numeric length, content, epoch/ref or expiry.
        if type(chunk) is bytes:
            self._diagnostic_count = min(MAX_RESPONSES, self._diagnostic_count + len(chunk))

    def diagnostics(self):
        return dict(byte_count=self._diagnostic_count, closed=self.channel_closed,
                    owner=self.remote_exit is not None, length_class=self._length_class,
                    opcode_class=self._opcode_class, rejected_at=self._rejected_at)

    def _fault(self, code, at):
        if self._rejected_at is None:
            self._rejected_at = at
        raise TransportFault(code)

    def feed(self, chunk):
        self.count_discarded(chunk)
        if not isinstance(chunk, bytes) or self._total + len(chunk) > MAX_RESPONSES:
            self._fault('LIMIT_EXCEEDED', 'LIMIT')
        self._total += len(chunk)
        self._buffer.extend(chunk)
        while len(self._buffer) >= 4:
            length, = struct.unpack('>I', self._buffer[:4])
            self._length_class = {7: 'BODY_7', 23: 'BODY_23', 46: 'BODY_46'}.get(length, 'INVALID')
            self._opcode_class = None
            if length not in (7, 23, 46):
                self._fault('INVALID_FRAME', 'LENGTH')
            if len(self._buffer) < length + 4:
                return
            body = bytes(self._buffer[4:length + 4])
            del self._buffer[:length + 4]
            magic, version, opcode = struct.unpack('>IBB', body[:6])
            self._opcode_class = {0x81: 'REF', 0x82: 'CLOSED', 0x83: 'OWNER'}.get(opcode, 'OTHER')
            if magic != MAGIC:
                self._fault('INVALID_FRAME', 'MAGIC')
            if version != 1:
                self._fault('INVALID_FRAME', 'VERSION')
            if opcode == 0x81 and length == 46 and self.reference is None and not self.channel_closed:
                expires, = struct.unpack('>q', body[38:46])
                try:
                    self.reference = PrivateRef(body[6:22], body[22:38], expires)
                except TransportFault:
                    self._fault('INVALID_FRAME', 'ORDER_OR_FIELDS')
            elif opcode == 0x82 and body[6:] == b'\x01' and not self.channel_closed:
                self.channel_closed = True
            elif (opcode == 0x83 and length == 23 and self.channel_closed
                  and self.remote_exit is None and body[6:22] == self._instance and body[22] in (0, 1)):
                self.remote_exit = body[22]
            else:
                self._fault('INVALID_FRAME', 'ORDER_OR_FIELDS')

    def eof(self):
        if self._buffer or not self.channel_closed or self.remote_exit is None:
            self._fault('CHANNEL_LOST', 'EOF')
