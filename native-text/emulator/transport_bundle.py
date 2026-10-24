"""Strict READY progress and post-cleanup FINAL Bundle parsing, without raw output."""
from transport_wire import REASONS, TransportFault

PREFIX = 'tv_native_'
PROTOCOL = PREFIX + 'lab_protocol_v1'
INSTANCE = PREFIX + 'lab_instance_v1'
READY = PREFIX + 'lab_ready_v1'
TEXT = PREFIX + 'text_v1'
CLEANUP = PREFIX + 'cleanup_v1'
ENTERED = PREFIX + 'entered_v1'
REASON = PREFIX + 'reason_v1'
READY_KEYS = frozenset((PROTOCOL, INSTANCE, READY))
FINAL_KEYS = frozenset((PROTOCOL, INSTANCE, TEXT, CLEANUP, ENTERED, REASON))


class Instrumentation:
    MAX_BYTES = 8192
    MAX_LINE = 512

    def __init__(self, instance):
        if len(instance) != 32 or any(c not in '0123456789abcdef' for c in instance):
            raise TransportFault('INVALID_BUNDLE')
        self.instance = instance
        self._buffer = bytearray()
        self._total = 0
        self._status = {}
        self._result = {}
        self.ready = False
        self.final = None

    def _field(self, fields, allowed, raw):
        key, separator, value = raw.partition('=')
        if not separator or key not in allowed or key in fields:
            raise TransportFault('INVALID_BUNDLE')
        if key == PROTOCOL and value != '1' or key == INSTANCE and value != self.instance:
            raise TransportFault('INVALID_BUNDLE')
        if key == READY and value != 'READY':
            raise TransportFault('INVALID_BUNDLE')
        if key == TEXT and value not in ('refused', 'uncertain', 'verified'):
            raise TransportFault('INVALID_BUNDLE')
        if key == CLEANUP and value not in ('VERIFIED', 'UNVERIFIED', 'NOT_APPLICABLE'):
            raise TransportFault('INVALID_BUNDLE')
        if key == ENTERED and value not in ('NO', 'YES') or key == REASON and value not in REASONS:
            raise TransportFault('INVALID_BUNDLE')
        fields[key] = value

    def _line(self, raw):
        line = None
        try:
            line = raw.decode('ascii', errors='strict')
        except UnicodeError:
            pass
        if line is None or self.final is not None:
            raise TransportFault('INVALID_BUNDLE')
        if line.startswith('INSTRUMENTATION_STATUS: '):
            if self.ready or self._result:
                raise TransportFault('INVALID_BUNDLE')
            self._field(self._status, READY_KEYS, line[24:])
        elif line == 'INSTRUMENTATION_STATUS_CODE: 1':
            if self.ready or self._status.keys() != READY_KEYS or self._result:
                raise TransportFault('INVALID_BUNDLE')
            self.ready = True
        elif line.startswith('INSTRUMENTATION_RESULT: '):
            self._field(self._result, FINAL_KEYS, line[24:])
        elif line == 'INSTRUMENTATION_CODE: 0':
            if self._result.keys() != FINAL_KEYS or self._status and not self.ready:
                raise TransportFault('INVALID_BUNDLE')
            if self._result[CLEANUP] == 'NOT_APPLICABLE':
                raise TransportFault('INVALID_BUNDLE')
            if self._result[ENTERED] == 'YES' and not self.ready:
                raise TransportFault('INVALID_BUNDLE')
            if self._result[CLEANUP] == 'UNVERIFIED' and self._result[TEXT] != 'uncertain':
                raise TransportFault('INVALID_BUNDLE')
            if (self._result[ENTERED] == 'YES' and self._result[TEXT] == 'refused'
                    and self._result[REASON] != 'ENGINE_REFUSED'):
                raise TransportFault('INVALID_BUNDLE')
            if self._result[TEXT] == 'verified' and (self._result[ENTERED] != 'YES'
                                                    or self._result[REASON] != 'NONE'
                                                    or self._result[CLEANUP] != 'VERIFIED'):
                raise TransportFault('INVALID_BUNDLE')
            self.final = dict(self._result)
        else:
            raise TransportFault('INVALID_BUNDLE')

    def feed(self, chunk):
        if not isinstance(chunk, bytes) or self._total + len(chunk) > self.MAX_BYTES:
            raise TransportFault('LIMIT_EXCEEDED')
        self._total += len(chunk)
        for value in chunk:
            if value == 10:
                self._line(bytes(self._buffer))
                self._buffer.clear()
            else:
                self._buffer.append(value)
                if len(self._buffer) > self.MAX_LINE:
                    raise TransportFault('LIMIT_EXCEEDED')

    def eof(self):
        if self._buffer or self.final is None:
            raise TransportFault('INVALID_BUNDLE')
