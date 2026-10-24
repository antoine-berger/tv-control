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
        encoded = units = None
        if isinstance(text, str):
            try:
                encoded = text.encode('utf-8', errors='strict')
                units = len(text.encode('utf-16-be', errors='strict')) // 2
            except UnicodeError:
                pass
        if encoded is None or units is None:
            raise TransportFault('INVALID_FRAME')
        if not encoded or len(encoded) > 2048 or units > 256:
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

    def feed(self, chunk):
        if not isinstance(chunk, bytes) or self._total + len(chunk) > MAX_RESPONSES:
            raise TransportFault('LIMIT_EXCEEDED')
        self._total += len(chunk)
        self._buffer.extend(chunk)
        while len(self._buffer) >= 4:
            length, = struct.unpack('>I', self._buffer[:4])
            if length not in (7, 23, 46):
                raise TransportFault('INVALID_FRAME')
            if len(self._buffer) < length + 4:
                return
            body = bytes(self._buffer[4:length + 4])
            del self._buffer[:length + 4]
            magic, version, opcode = struct.unpack('>IBB', body[:6])
            if magic != MAGIC or version != 1:
                raise TransportFault('INVALID_FRAME')
            if opcode == 0x81 and length == 46 and self.reference is None and not self.channel_closed:
                expires, = struct.unpack('>q', body[38:46])
                self.reference = PrivateRef(body[6:22], body[22:38], expires)
            elif opcode == 0x82 and body[6:] == b'\x01' and not self.channel_closed:
                self.channel_closed = True
            elif (opcode == 0x83 and length == 23 and self.channel_closed
                  and self.remote_exit is None and body[6:22] == self._instance and body[22] in (0, 1)):
                self.remote_exit = body[22]
            else:
                raise TransportFault('INVALID_FRAME')

    def eof(self):
        if self._buffer or not self.channel_closed or self.remote_exit is None:
            raise TransportFault('CHANNEL_LOST')
