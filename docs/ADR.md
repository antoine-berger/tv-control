# Architecture decisions

## 001 — Dependency direction

The Rust domain has no dependencies. The application depends only on the domain
and defines `Control`, `DeviceControl`, `MediaCatalogue`, `StateStore`, `Clock`
and `Cancellation` ports. Adapters implement those ports and the binary composes
them. This keeps protocol, filesystem and environment details out of use cases.
The [architecture check](https://github.com/antoine-berger/tv-control/blob/main/scripts/check_architecture.py) enforces the inner
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
Unsafe code is confined to Unix I/O, process, ownership and terminal operations
in the adapters/CLI; there is no unsafe domain/application code. Process-group
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
store credentials, discovery snapshots or authorization claims. Starting or resuming setup requests bounded discovery on the chosen LAN;
there is no extra search or local draft save confirmation. Saved interfaces and target availability
are revalidated. Input-time advertisement expiry triggers one bounded refresh on
the same interface/address, control protocol and service instance. Dynamic ports
may change only with this fresh advertisement evidence; failure offers explicit
retry in the wizard. Saving configuration grants no device access. Recovery preserves corrupt data and handles recognized temporary
hard links from interrupted initial publication. Content and open-file identity
protect against accidental replacement or in-place edits during a session.

Numeric discovery OS errors cross the port as optional failure evidence. They
do not establish a cause; raw OS messages and packet payloads are not logged.

## 011 — Complete onboarding evidence

The zero-argument installer composes configuration, private device access,
official tunnel supervision and cooperative conversation verification. The
composition is covered by isolated native fixtures; actual hosted and hardware
qualification remains separate from these tests and from source publication.

The pipeline advances through configuration, ADB consent, an owned ADB runtime,
a read-only device observation, local MCP discovery, tunnel consent, tunnel
readiness and external conversation verification. Each port returns evidence or
a distinct pending, blocked, rejected, cancelled, expired or error outcome.
Evidence is bound to configuration revision, device, runtime generation and,
where applicable, tunnel generation and a new challenge. Resume re-observes the
prerequisites; a saved stage must never serve as an authorization grant. An
approval wait cannot make expired evidence valid. Fixtures produce
`SimulationComplete`, never `Ready`.

Native integration uses the official tunnel-client managed runtime commands,
including `runtimes connect` with an existing tunnel ID and an environment secret
reference, followed by structured `runtimes status` checks. Reusing an existing
tunnel avoids asking for an administrator key. Creating a tunnel is supported by
the official administrative CLI, but needs a separate explicit approval and the
required organization permissions. The runtime must never receive an admin key.
No background supervision path uses nohup/disown. The official manager supervises
the bridge, and the bridge owns the private ADB child and its cleanup. The stable
installed launcher, configuration snapshot and owned alias bind runtime identity.
Starting access and creating the private identity require explicit approval.

Local MCP initialization, tool discovery and tunnel health are separate checks.
None proves that an intended chat has installed or selected the plugin. The
official workflow still includes account/workspace approvals and plugin
installation/selection. The final protocol must guide a harmless Observe call
from that conversation using a fresh challenge tied to the selected installation
and configuration. A local receipt, `clientInfo.name`, a copied health response or
pressing Enter is insufficient. The supported tunnel metadata supplies no signed
conversation identity. Any cooperative human confirmation of the selected chat
must be described as such; it must not be called cryptographic attestation.
Until that external workflow supplies its evidence, the pipeline stays pending.

Each dependent stage receives a context bounded by the validity of its required
evidence. Execution mode is fixed before composition: the agent fixture route
contains only simulated ports, while the native route rejects simulated evidence.
An expired approval flow stops before dependent dispatch, not just before Ready.
After cooperative code confirmation, fresh runtime health and a new TV observation
are required. Every bridge start publishes a private random process generation;
the bridge, challenge, response and final confirmation must agree on that token.
Stable installation and tunnel IDs cannot substitute for a process generation.
Changed configuration requires explicit approval, a verified stop of the exact
old owned runtime, a preserved old intent and an atomic replacement with a new
revision. Pending proofs are invalidated and all access approvals run again.
Logs accept only typed event codes and durations, bounded to 64 KiB.
Credentials are entered without echo and passed only to the official runtime's
environment; checkpoints contain neither credentials nor reusable proof claims.

Release review covers child cleanup, configuration binding, consent, secret
handling and replay. A real conversation call and physical TV qualification must
still be recorded before declaring that installation connected or making latency
claims. Missing account/workspace permissions remain a platform blocker.

Sources inspected for this design:

- [Secure MCP Tunnels](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)
- [Connect a plugin to ChatGPT](https://developers.openai.com/plugins/deploy/connect-chatgpt)
- [Official tunnel-client onboarding](https://github.com/openai/tunnel-client/blob/master/docs/onboarding.md)
- [Managed runtime commands](https://github.com/openai/tunnel-client/blob/master/plugins/tunnel-mcp/skills/tunnel-mcp/references/runtime-flows.md)
- [Local health evidence](https://github.com/openai/tunnel-client/blob/master/docs/health.md)
- [Tunnel wire protocol](https://github.com/openai/tunnel-client/blob/master/docs/protocol.md)
