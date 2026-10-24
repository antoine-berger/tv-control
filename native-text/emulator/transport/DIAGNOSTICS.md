# Secondary LAB diagnostics v1

These diagnostics never authorize an operation or replace remote cleanup proof.
Every nonempty stderr still makes the Host reject with STDERR_OUTPUT/Unknown
(or preserves an earlier sticky failure). Nothing is accepted by magic scanning,
stdout resynchronization, an exit code, or a recognized diagnostic line.

The client failure-only stderr grammar is exactly one ASCII line, including its
single LF, at most 160 bytes total:

`TV_NATIVE_CLIENT_FAILURE_V1 phase=<PHASE> reason=<REASON> cleanup=<CLEANUP>\n`

PHASE is one of ARGUMENTS, LOCAL_UID, CONNECT, STDIO_PREPARE, WORKER_START,
READ_RESPONSE, VALIDATE_RESPONSE, FORWARD_INPUT, STOP_JOIN, INPUT_STOP,
SOCKET_CLOSE, WRITE_CLOSED, OUTPUT_STOP. REASON is an existing LabProtocol.Reason
other than NONE. CLEANUP is VERIFIED or UNVERIFIED. There is no free text,
exception class/message/stack, path, UID, instance, ref, epoch, expiry or payload.
The Java writer attempts a single nonblocking Os.write after cleanup, with no
poll/retry/new budget. A short write or no output is possible and never proves
success. Java source ownership and review remain separate from the host parser.

The host checks the 160-byte bound before appending to its private in-memory
buffer. At EOF it erases the buffer and returns only ABSENT, VALID, UNKNOWN,
TRUNCATED, DUPLICATE or OVERSIZE. Only VALID also returns the three closed enum
fields. An unfinished stream is TRUNCATED; more than one line is DUPLICATE;
exceeding the bound is sticky OVERSIZE. No raw stderr is stored in a result,
receipt, file, logger, exception or public representation.

Outcome diagnostics contain only:

- server_ready: whether strict correlated READY was parsed;
- server_final: null unless FINAL passed the strict Bundle parser and EOF,
  otherwise its closed text/cleanup/entered/reason enums, without instance;
- response: saturated byte_count from 0 through 88, accepted CLOSED/owner bools,
  length_class BODY_7/BODY_23/BODY_46/INVALID (or null), opcode_class
  REF/CLOSED/OWNER/OTHER (or null), and rejected_at LIMIT/LENGTH/MAGIC/VERSION/
  ORDER_OR_FIELDS/EOF (or null);
- client_stderr: the closed collector result, or null when no client collector
  exists. A recognized line remains untrusted, secondary failure evidence.

No arbitrary integer prefix length or opcode value is published. Byte counts
saturate even if discarded output is larger. After a parser fault, later bytes
may update only that saturated count; they are not scanned or reinterpreted.
These projections distinguish an owner-before-CLOSED failure from malformed
framing while leaving every existing acceptance condition unchanged.
