"""Secondary fixed diagnostics only. Recognition never authorizes stderr output."""
import re
from transport_wire import REASONS

PREFIX = 'TV_NATIVE_CLIENT_FAILURE_V1'
PHASES = frozenset('ARGUMENTS LOCAL_UID CONNECT STDIO_PREPARE WORKER_START READ_RESPONSE VALIDATE_RESPONSE FORWARD_INPUT STOP_JOIN INPUT_STOP SOCKET_CLOSE WRITE_CLOSED OUTPUT_STOP'.split())
FAILURE_REASONS = REASONS - {'NONE'}
LIMIT = 160


class ClientFailure:
    """At most 160 private in-memory bytes, erased on classification; no raw getter."""
    def __init__(self):
        self._pending = bytearray()
        self._finished = False
        self._oversize = False
        self._after_eof = False
        self._result = None

    def __repr__(self):
        return '<ClientFailureDiagnostic>'

    def feed(self, chunk):
        if self._finished:
            self._after_eof = True
            return
        if self._oversize:
            return
        # Test the bound BEFORE copying/appending any untrusted chunk.
        if type(chunk) is not bytes or len(chunk) > LIMIT - len(self._pending):
            self._oversize = True
            self._pending.clear()
            return
        self._pending.extend(chunk)

    def eof(self):
        if self._finished:
            return
        self._finished = True
        category = 'UNKNOWN'
        fields = None
        if self._oversize:
            category = 'OVERSIZE'
        elif not self._pending:
            category = 'ABSENT'
        elif self._pending.count(10) > 1 or 10 in self._pending[:-1]:
            category = 'DUPLICATE'
        elif self._pending[-1] != 10:
            category = 'TRUNCATED'
        elif all(value < 128 for value in self._pending):
            # Only this bounded transient ASCII string is decoded. Arbitrary
            # values never enter an error, public result, log, or retained field.
            line = self._pending.decode('ascii')
            match = re.fullmatch(PREFIX + r' phase=([A-Z_]+) reason=([A-Z_]+) cleanup=(VERIFIED|UNVERIFIED)\n', line)
            if match and match[1] in PHASES and match[2] in FAILURE_REASONS:
                category = 'VALID'
                fields = dict(phase=match[1], reason=match[2], cleanup=match[3])
        self._pending.clear()
        self._result = dict(category=category, fields=fields)

    def public(self):
        if self._after_eof:
            return dict(category='DUPLICATE', fields=None)
        if self._oversize:
            return dict(category='OVERSIZE', fields=None)
        if not self._finished:
            return dict(category='TRUNCATED', fields=None)
        # Return a copy, never the private buffer or mutable internal result.
        fields = self._result['fields']
        return dict(category=self._result['category'], fields=None if fields is None else dict(fields))
