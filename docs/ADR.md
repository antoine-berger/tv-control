# Architecture decisions

## 001 — Rust and strict dependency direction

Use Rust for a native controller without a GC or embedded browser/model. The
domain has no dependencies. The application defines `Control`, `DeviceControl`,
`MediaCatalogue`, `StateStore`, `Clock` and `Cancellation` ports. Adapters implement
them; the binary composes them. A metadata check enforces the inner dependencies.
Resource suitability is measured, not inferred from the language alone.

## 002 — Reuse maintained transports

Invoke configured ADB and curl executables. ADB authentication, smart sockets and
TLS are not reimplemented. curl supplies system TLS and is optional for an ADB-only
installation. Their RSS, CPU, binaries and any ADB daemon/tunnel are separate costs
from the Rust controller. The router must provide compatible maintained versions.
Do not assume an old vendor ADB supports modern pairing or TLS.

## 003 — A bounded synchronous core

Use synchronous ports, bounded MCP workers and nonblocking Unix subprocess pipes.
This avoids an async runtime and reader thread per pipe. libc is confined to
nonblocking flags and process-group cleanup in the process adapter; unsafe calls
have ownership comments. No unsafe code is in domain/application. Process groups
are cleanup, not a security boundary for adversarial executables.

## 004 — Deadline is an input contract

The client supplies an absolute expiry. Convert the remaining budget once to a
monotonic deadline and check before each operation and after reads. No per-step
timeout resets the budget. Reservations refuse contention instead of queueing;
future locking adapters must remain within the same Context. The controller cannot
recover a missing or fabricated user-submission timestamp from a hosted chat.
Subprocess spawn and OS scheduling themselves are not hard real-time primitives.

## 005 — Observation rather than dispatch proves success

A 204 or successful subprocess exit is an acknowledgement only. Defined state
postconditions must be observed before expiry. Jellyfin resume-media requires
matching media and position progress. No native app launch implies content playback.
After possible dispatch, cancellation/transport loss/timeout is `outcome_unknown`.
There is no automatic repeat, fallback key sequence or forced rollback.

## 006 — Bounded volatile coordination, honestly scoped

An atomic reservation covers request identity and per-device exclusion. The memory
store retains completed receipts and never evicts in-flight entries. Duplicated
in-flight calls return busy; completed identical calls replay a marked receipt.
Changed actions using an ID conflict. A full store refuses new operations.
The cache is process-local and is lost on restart. Durable idempotency requires a
future journal/lease adapter and is not claimed by this release. Device locks also
do not coordinate unrelated controllers or physical remotes.

## 007 — Configuration carries installation details, not executable logic

Use strict JSON with unique aliases and explicit references between devices,
applications, servers, media and workflows. The CLI loads and validates it before
composition. Protocol paths, enum names and validation bounds live in code; there
are no personal device addresses, accounts, titles or filesystem defaults in the
domain. Workflows currently expand to one configured resume-media intent.
New behavior requires code and contract tests, not executable strings in JSON.

## 008 — Narrow transport, separately authorized deployment

MCP stdio is the first incoming adapter, with a CLI for scripts and diagnostics.
There is no network listener or public endpoint. ChatGPT hosted access needs a
separate supported connection and permissions. Tunnel setup, service installation,
tokens, pairing and router configuration are out of the executable's control path.
No SLO for hosted transport is inferred from local fixture measurements.

## 009 — Reuse rather than claim invention

[Browser Use](https://github.com/browser-use/browser-use) demonstrates optional
vision, grouped actions and reusable sessions. [agent-browser](https://github.com/vercel-labs/agent-browser)
demonstrates a compact snapshot/batch interface. [brickproof-adb](https://github.com/st412m/brickproof-adb)
already implements UI dumps and selector-based DPAD navigation. This project does
not invent those capabilities, embed them or claim their performance on a TV.
The MVP focuses on typed media intents, deadlines, qualified adapters and evidence.
Any source reuse must preserve its upstream licence and be reviewed separately.
