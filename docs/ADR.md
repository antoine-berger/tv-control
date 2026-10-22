# Architecture decisions

## 001 — Dependency direction

The Rust domain has no dependencies. The application depends only on the domain
and defines `Control`, `DeviceControl`, `MediaCatalogue`, `StateStore`, `Clock`
and `Cancellation` ports. Adapters implement those ports and the binary composes
them. This keeps protocol, filesystem and environment details out of use cases.
The [architecture check](../scripts/check_architecture.py) enforces the inner
crate boundaries. Resource suitability still requires measurement.

## 002 — External maintained transports

Use configured ADB and curl executables rather than reimplement authentication,
smart-socket device transport or TLS. Their binaries, processes and any daemon or
tunnel have costs separate from the controller. ADB includes a read-only version
probe but remains manager-capable; [dedicated daemon ownership](ADAPTERS.md#adb-daemon-ownership)
is required. Router packages must be qualified rather than assumed compatible.

## 003 — Bounded synchronous execution

Use synchronous application ports, bounded MCP workers and nonblocking Unix
subprocess/stdio I/O. This avoids an async runtime and a reader thread per pipe.
Unsafe code is confined to adapter-level Unix descriptor/process operations, with
ownership comments; there is no unsafe domain/application code. Process-group
cleanup limits local work but is not a sandbox for hostile executables.

## 004 — Caller-owned deadline

Accept an absolute expiry and convert its remaining budget once to monotonic
time. Check it throughout the operation; do not reset timeouts per step.
Reservations refuse contention rather than queueing mutations. Hosted-chat
submission time must come from the caller. Process creation and OS scheduling are
not hard real-time primitives. See [deadlines](CLI.md#deadlines-and-retries).

## 005 — State evidence determines success

A subprocess exit or HTTP acknowledgement does not prove the requested effect.
Observe the defined postcondition before expiry. App foreground state proves app
launch only, and Jellyfin playback needs position evidence. After possible
dispatch, cancellation or timeout is uncertain. Do not automatically retry, send
fallback keys or force rollback. See [playback evidence](ADAPTERS.md#playback-evidence).

## 006 — Volatile coordination

Atomically reserve request identity and a physical-device coordination group.
Retain bounded completed receipts without evicting live entries. Replay completed
identical calls with a cache marker; reject conflicts and capacity exhaustion.
State is process-local and lost on restart. Durable idempotency and cross-process
coordination require future journal/lease adapters. Physical remotes remain outside
these locks.

## 007 — Declarative configuration

Use strict JSON, unique aliases and explicit device/app/server/media/workflow
references. Configuration contains installation details and secret references,
not executable logic. Workflows expand to one configured series-resume action.
New behavior requires code and contract tests. See the
[configuration reference](CONFIGURATION.md).

## 008 — Local transport first

Provide CLI and MCP stdio adapters without a network listener. Hosted access,
tunnels, installation, pairing, persistent credentials and router configuration
need separate integration work and authorization. Local fixture performance does
not establish an end-to-end service-level guarantee. See [MCP integration](MCP.md).

## 009 — Existing automation work

[Browser Use](https://github.com/browser-use/browser-use),
[agent-browser](https://github.com/vercel-labs/agent-browser) and
[brickproof-adb](https://github.com/st412m/brickproof-adb) are related automation
projects. Their browser/vision or UI-navigation capabilities are not embedded
backends here, and their performance cannot be attributed to this controller.
Any future source reuse needs a separate license and integration review.

## 010 — Resumable local setup

The shell owns download, integrity checks and version activation. The Rust wizard
owns interface/display selection and configuration; the checkpoint adapter owns
private atomic persistence and process-held locking. The normal installer has no
arguments and enters that wizard after confirmation, reusing an intact installation
or updating an older version. Advanced noninteractive behavior stays explicit.

Checkpoints store local choices and configuration progress only. They do not
store credentials, discovery snapshots or authorization claims. Network consent
is renewed before a resumed discovery; saved interfaces and target availability
are revalidated. Recovery preserves corrupt data and handles recognized temporary
hard links from interrupted initial publication. Content and open-file identity
protect against accidental replacement or in-place edits during a session.

Numeric discovery OS errors cross the port as optional failure evidence. They
do not establish a cause; raw OS messages and packet payloads are not logged.
