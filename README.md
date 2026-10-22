# tv-control

A small Rust TV intent controller with a CLI and a narrow MCP stdio server.
There is no embedded model, browser, network listener, installer, discovery scan,
or automatic authentication. Device identities, applications, media mappings,
executables, resource limits and workflows belong in validated configuration.

**Status: early MVP.** Native macOS/Linux and loopback fixtures are the initial
verification surfaces. An ARM64 musl build is a packaging candidate, not proof of
compatibility with any installed router firmware. There is no ten-second
user-to-TV guarantee and no installed ChatGPT plugin or tunnel.

## Architecture

```text
CLI / MCP stdio                 incoming adapters
        | Control port
application service             deadline, coordination, postconditions
        | outbound ports
device control | catalogue | state store | clock | cancellation
        |
ADB executable | Jellyfin HTTP/curl | bounded memory state | system clock

domain <- application <- adapters <- composition/CLI
```

`tv-domain` has no dependencies; `tv-application` depends only on the domain.
The application has no JSON, ADB, HTTP, filesystem, environment or protocol
knowledge. `scripts/check_architecture.py` enforces this dependency direction.
The inbound transport port is `Control`; device, catalogue, state, clock and
cancellation ports are defined in the application crate.

## Build and test

Requires Rust, Python 3 for integration tests, and Unix (Linux or macOS). Runtime
adapters use an explicitly configured maintained ADB executable and/or curl.
No Rust async runtime or HTTP/TLS stack is embedded. The only direct third-party
crates are Serde, serde_json and libc; Cargo.lock pins the dependency graph.

```sh
cargo build --locked --release
cargo test --locked --workspace
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
python3 scripts/check_architecture.py
python3 -W error::ResourceWarning scripts/test_mcp.py target/release/tv-control
target/release/tv-control validate examples/config.json
```

The public example uses documentation-only addresses, fictitious identities and
an invalid example domain. It does not connect to a device when validated.
Copy it to an ignored `*.local.json` file and explicitly configure your own
installation. Keep real configuration outside the repository when possible.

## CLI

```sh
tv-control validate config.local.json
tv-control capabilities config.local.json
tv-control execute config.local.json request.local.json
tv-control execute config.local.json -
tv-control workflow config.local.json continue_series request_001 8000
tv-control mcp config.local.json
```

`execute` reads a bounded JSON request from a file or stdin. The shape is:

```json
{
  "request_id": "unique_operation_id",
  "device": "screen",
  "action": { "type": "observe" },
  "expires_at_unix_ms": 0
}
```

The zero expiry above is deliberately expired. The **calling client** must stamp
a real absolute expiry when the user submits the command. The controller never
renews it on arrival. A trusted integration must account for time spent in the
chat, model, transport and queue. `workflow` stamps its budget when that CLI
process runs; this does **not** measure or bound preceding chat latency.

Actions are `observe`, `pause`, `resume`, `open_app` with an `app` alias,
`set_volume` with an `index`, and `resume_media` with a `media` alias. Unknown
fields, free-form shell, URLs, hosts, packages or media titles are not tool
arguments. The MVP workflow is a configured series resume, not an arbitrary
UI macro or executable recipe. Exit codes: 0 observed/verified, 2 unsuccessful
receipt, 1 input/configuration/transport startup error.

## MCP and host integration

The server implements the negotiated MCP `2025-11-25` stdio subset: initialize,
initialized notification, ping, tools/list, tools/call and cancellation
notifications. It offers `tv_capabilities` and `tv_control`. JSON-RPC is one
message per line; stdout contains protocol messages only. Oversize frames are
rejected and terminate the connection. Workers and response state are bounded.

`examples/mcp.json` is a generic desktop client configuration example. Hosts have
different import formats; it is not an installed plugin. A hosted chat cannot
reach a local stdio process merely because a desktop client can. A separately
authorized private MCP bridge/tunnel and workspace permissions may be required.
No bridge, token, pairing, daemon or router service is provisioned by this repo.

## Actual adapter scope

| Adapter | Implemented | Qualification and limits |
| --- | --- | --- |
| ADB executable | Explicit existing target; state, TV app launch, targeted media pause/play; optional calibrated music stream index | Existing authorized server required. No server start, connect, pairing, key generation, installation or scan. Android vendor output must match parsers. |
| Jellyfin HTTP | Patched-server check, exact user/device session selection, pause/unpause, configured series resume at stored ticks, postcondition reads | Mock integration qualified first. Requires an existing credential and compatible client. No library mutation or token creation. |
| YouTube / YouTube Music | Generic app launch and native media controls only when configured and supported | No search, lyrics, song/video mode, account navigation or service-specific Cast adapter. |
| Cast / Remote v2 / UIAutomator | Extension points only | Not implemented; not advertised as working backends. |

ADB `set_volume` is disabled unless `volume_max_index` is explicitly calibrated.
Its unit is the Android music stream index, **not TV percent, an external audio
device level or the number displayed by a television**. The observed maximum
must match configuration. It is not a promise that index 40 means TV volume 40.

Jellyfin uses `/UserItems/Resume` scoped to configured user and series, then
`/Sessions/{id}/Playing` with `playCommand`, `itemIds` and `startPositionTicks`.
A 204 response means dispatch only. Resume-media success requires matching item
identity and two observations showing increasing position. Resume with a server
position also requires progress, not only `IsPaused=false`. This is evidence of
client-reported progress, not proof of a physical first video frame or audio.
Android TV clients can implement Pause/Unpause as toggles: state is checked
immediately before sending once. A simultaneous physical remote action can
still race this check; there is no compare-and-set API.

## Deadlines, cancellation and duplicates

* One absolute deadline covers reservations, reads, catalogue lookup, subprocess
  input/output, dispatch and verification. It is converted to monotonic time.
  Device contention returns `busy` rather than queueing behind another operation.
* Subprocess pipes are nonblocking and bounded. Each invocation owns a Unix
  process group, terminated on timeout/cancellation/exit cleanup. Executables
  are trusted administrator configuration, not a sandbox for hostile programs.
* Cancellation before dispatch returns `cancelled`; after possible dispatch it
  returns `outcome_unknown`. Killing a local process does not undo a TV command
  or guarantee that a buffering player will not start later.
* Request IDs deduplicate the same device/action within this **running process**.
  A different action with the same ID is rejected. Entries are bounded and live
  entries are never evicted. Capacity exhaustion fails closed. Cached receipts
  are marked and replay old evidence; they are not fresh device observations.
* State is currently volatile. Restart/crash loses the deduplication cache.
  Do not automatically replay uncertain mutations across a restart. Persistent
  journaling/leases are future StateStore adapters, not implemented guarantees.
* `observed` is a state-read receipt; `verified` proves the defined postcondition;
  `expired`, `cancelled`, `busy`, `unsupported`, `failed`, `invalid` and
  `outcome_unknown` are not successes. An app in the foreground proves app launch
  only, not playback of the requested content.

## Security

Jellyfin session-control authorization was affected by
[GHSA-4vx8-xhc9-qg6x](https://github.com/jellyfin/jellyfin/security/advisories/GHSA-4vx8-xhc9-qg6x).
The adapter rejects pre-release versions and server versions below patched 12.0.
This baseline check does not establish that a server has no other vulnerabilities.
Exact configured user and device matching is enforced independently.

Credentials are environment **references** in configuration. Values are read only
inside the HTTP adapter and supplied to curl on stdin, never in process arguments,
tool output or raw stderr. Redirects, curl configuration inheritance and URL
globbing are disabled. TLS validation is enabled; cleartext HTTP requires explicit
configuration. No secure erasure of process memory is claimed.

ADB debug authorization remains broad even behind a narrow tool schema. Legacy
TCP debugging is not encrypted by RSA authorization; an Internet tunnel does not
encrypt that separate LAN leg. Keep the ADB server on loopback and select an
explicit target. Run under a dedicated unprivileged account where practical.
Never publish real configuration, keys, dumps, captures or viewing history.

## Router and performance

See [router packaging](docs/ROUTER.md), [acceptance protocol](docs/ACCEPTANCE.md),
[performance scope](benchmarks/README.md), [ADRs](docs/ADR.md) and
[delivery checklist](PLAN.md). No router deployment is part of this MVP.

The ten-second target is **user submission → requested physical effect and
confirmation**, not CLI runtime. Outages, model scheduling, auth dialogs, media
buffering and hosted-chat transport prevent an unconditional upper bound. Record
late/unknown outcomes as failures of the target, never as success at ten seconds.
