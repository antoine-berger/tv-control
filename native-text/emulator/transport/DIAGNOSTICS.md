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


## Secondary monotonic measurements

Timing is diagnostic only. It is never used to decide success/refusal, compute or
renew a deadline, skip/reorder a guard, dispatch another command, or infer a
remote timestamp. The two modules use the same host monotonic clock; tests may
inject that same clock into both. The origin is the existing Host-run clock read
that establishes its unchanged deadline. No absolute clock value is published.

`timing` contains the fixed origin label HOST_RUN_MONOTONIC, a `host` map with
ready_received_ms, host_client_guard_start_ms, host_client_guard_end_ms,
client_call_start_ms, client_call_end_ms, and a `launcher` map with guard_ms,
popen_ms, incomplete. An outer incomplete bool is sticky for missing/invalid
observations. Every numeric value is an integer in 0..15000, or null when not
reached/invalid. Python round selects the nearest millisecond (ties to even).
Values above 15000 ms saturate and mark incomplete: saturation is not the actual
duration beyond that point. Bool, non-finite, backwards or throwing clock values
produce no numeric value and mark incomplete without replacing an operation's
primary exception or verdict. The launcher property returns a closed copy.

READY means the first validated READY group observed by the Host, not a device
uptime, the remote accept start, or a synchronized host/device timestamp. Guard
and client-call ends are sampled in finally, including failure paths. popen_ms
covers entry to Popen through the immediate PipedChild registration callback;
registration occurs first, before its diagnostic sample. This includes initial
PipedChild allocation/registration overhead and is not a pure syscall measure.
No argument, path, instance, payload, exception text or identity is measured.

A synthetic guard test injects 200 ms per metadata command, 50 ms per lsof and
10 ms per host query. The unchanged two-guard graph then costs 5780 ms (16 ADB
metadata commands, 38 lsof calls, 68 queries), beyond a 5000 ms accept window but
inside 15000 ms. These are injected costs, not measured runtime latencies and
not proof of the cause of any real CONNECT/TRANSPORT_LOST failure. Permissions,
startup failures and other connection failures remain possible.
