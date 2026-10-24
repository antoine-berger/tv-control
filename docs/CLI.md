# CLI reference

Examples assume a release build and commands run from the repository root.
Use `./target/release/tv-control --help` for the command summary or `--version`
for the package version. Commands taking `CONFIG` load and validate that file;
discovery and setup commands work before a configuration exists. For an installed
copy, replace `./target/release/tv-control` with
`"$HOME/.local/share/tv-control/tv-control"` (or your selected install prefix).

## Commands

```text
tv-control interfaces [--json]
tv-control discover [--interface NAME] [--json]
tv-control setup [--json|--dry-run]
tv-control doctor CONFIG [--probe] [--device ALIAS] [--json]
tv-control connect-chatgpt CONFIG [--tunnel-id ID] [--json]
tv-control validate CONFIG
tv-control capabilities CONFIG
tv-control execute CONFIG REQUEST.json|-
tv-control workflow CONFIG ALIAS REQUEST_ID TTL_MS
tv-control mcp CONFIG
tv-control onboard CONFIG [--json]
tv-control disconnect CONFIG [--json]
```

| Command | Behavior |
| --- | --- |
| `interfaces` | List candidate IPv4 LAN interfaces |
| `discover` | Search the selected LAN for unverified display advertisements |
| `setup` | Prepare/save private configuration; does not authorize the TV or start the controller |
| `doctor` | Check configuration offline; `--probe` requests a read-only device call |
| `connect-chatgpt` | Print a manual connection plan; do not create a tunnel or plugin |
| `validate` | Check configuration structure and references; print `{"valid":true}` |
| `capabilities` | List configured aliases and adapter actions without device access |
| `execute` | Execute one JSON request from a file, or from stdin with `-` |
| `workflow` | Expand a configured workflow to one series-resume request |
| `mcp` | Run the local [MCP stdio server](MCP.md) until its transport closes |
| `onboard` | Guide explicit access approvals, native checks, official tunnel supervision and cooperative conversation verification; JSON mode returns pending without granting access |
| `disconnect` | Explicitly stop only the exact owned managed controller; preserve configuration and TV trust |

Capability listing describes configured support; it does not test runtime
connectivity. `execute` input is bounded by `max_frame_bytes`.

## Requests

```json
{
  "request_id": "request_001",
  "device": "screen",
  "action": { "type": "observe" },
  "expires_at_unix_ms": 1
}
```

The timestamp above is intentionally expired. The caller must supply a real Unix
expiry in milliseconds when submitting a request. `request_id` and `device` are
1–64 ASCII letters, digits, underscores or hyphens. `device` names a configured
alias. Unknown request and action fields are rejected.

For a local read with an eight-second budget, after configuring an authorized
target:

```sh
python3 - <<'PY_REQUEST' | ./target/release/tv-control execute config.local.json -
import json, time, uuid
print(json.dumps({
    "request_id": uuid.uuid4().hex,
    "device": "screen",
    "action": {"type": "observe"},
    "expires_at_unix_ms": time.time_ns() // 1_000_000 + 8000,
}))
PY_REQUEST
```

This contacts the device. The timestamp measures local submission; a chat host
must preserve the earlier user-submission deadline instead of stamping a fresh
budget here.

| Action | JSON `action` object | Postcondition |
| --- | --- | --- |
| Observe | `{"type":"observe"}` | Read state; no mutation requested |
| Pause | `{"type":"pause"}` | Playback state is paused |
| Resume | `{"type":"resume"}` | Native playing state, or matching media with increasing position for position-based evidence |
| Open app | `{"type":"open_app","app":"player"}` | Resolved installed app (or legacy configured app) is in the foreground |
| Set volume | `{"type":"set_volume","index":5}` | Observed calibrated Android music stream index matches |
| Resume media | `{"type":"resume_media","media":"series"}` | Matching item, expected seek range and two observations with increasing position |

Action availability depends on the [adapter](ADAPTERS.md). With the explicit generic
ADB grant, app selectors may be observed names, package IDs or exact discovered
components. Without it, configured app aliases keep their restricted behavior.
Media arguments remain configured aliases. There is no free-form shell tool.
The generic `wake`, `navigate`, `search`, `activate` and `type_text` shapes in [MCP](MCP.md#generic-adb-reads-and-intents)
are also accepted by `execute`. `execute`, `workflow`, MCP and the managed bridge
share the same persistent mutation journal beside the configuration.
UI references are intentionally process-local: separate `execute` invocations
cannot reuse them. Reproduce reference-based flows from a terminal through one
persistent `mcp CONFIG` stdio session, issuing `tv_read` and then `tv_control` in
that same process. Use a fresh future expiry and a unique operation ID; the
examples in MCP deliberately contain expired placeholders.

## Receipts and exit codes

A legacy receipt contains `request_id`, `status`, `code`, `cached`, `local_elapsed_ms`,
`timing_scope` and an optional `observed` state. `code` explains the result;
`timing_scope` is `controller_local_not_user_to_tv`.

Observed state includes connection, active app alias, playback state and evidence
type, media ID, position ticks and volume index when available. Media positions
use 100 ns ticks. These fields can contain viewing information; keep real
receipts private.

| Status | Interpretation |
| --- | --- |
| `observed` | A state read completed |
| `verified` | The action's defined postcondition was observed before expiry |
| `invalid` | Invalid identity, request conflict or budget outside policy |
| `busy` | Device, request or bounded state capacity is occupied |
| `unsupported` | The adapter does not support the action |
| `expired` / `cancelled` | Stopped before possible mutation |
| `failed` | Failure classified before mutation |
| `outcome_unknown` | The action may have reached the device; success is unconfirmed |

CLI exit code `0` means `observed` or `verified`; `2` means an unsuccessful control
receipt. Input, configuration and transport startup errors exit `1` with a short
error code on stderr. `workflow` succeeds only with a `verified` receipt.

Verification is limited to the [reported evidence](ADAPTERS.md#playback-evidence).
It does not prove physical video or audio output.

## Deadlines and retries

One absolute deadline covers reservation, observation, catalogue lookup, dispatch,
subprocess I/O and verification. The controller converts remaining wall-clock
budget to monotonic time without renewing it. Already expired requests are
rejected; budgets above `max_budget_ms` are invalid. OS scheduling and process
creation are not hard real-time operations.

Request IDs deduplicate matching operations within the configured retention
window. A completed request replays its receipt with `cached: true`; that evidence
is historical. An in-flight duplicate returns busy. Reusing an ID for a different
operation is invalid. Retained and active entries are never evicted to admit a
new request; a full store returns busy.

`execute`, `workflow`, direct MCP and the managed bridge share the private durable
`.tv-control-operations` journal beside the configuration. Processes using that
same journal share physical-device resource exclusion, including generic and
legacy mutations. Completed receipts survive restarts; an interrupted operation
remains uncertain rather than authorizing another dispatch. This does not
coordinate physical remotes, independent controllers or configurations copied
into different state directories. Never retry an uncertain operation under a
fresh ID to bypass its journal entry.

Cancellation before dispatch returns `cancelled`. After possible dispatch,
cancellation or timeout returns `outcome_unknown`. Process cleanup cannot undo a
TV command or stop a player that buffers before starting. A lost response after
dispatch is also uncertain for the caller. Do not automatically replay an
uncertain mutation, including after a restart; inspect current state first.

## Discovery and setup

Prefer the [installer](INSTALLATION.md) for the complete guided journey. Direct
`setup` writes `tv-control.local.json` in the current directory by default;
the installer selects the private configuration directory instead.

| Options | Commands and meaning |
| --- | --- |
| `--json` | Structured output; setup previews without writing configuration unless `--confirm` is supplied. JSON mode never installs dependencies or grants device access. |
| `--dry-run` | `setup` and `discover` skip live discovery; use a fixture for an offline preview with candidates. `doctor --probe --dry-run` skips the probe. |
| `--fixture FILE` | `interfaces`, `discover`, `setup`: use recorded discovery data instead of a real network. |
| `--interface NAME` | `discover`, `setup`: select a listed interface (or `NAME@IPv4`). |
| `--duration-ms MS`, `--timeout-ms MS` | `discover`, `setup`: receive window and overall search timeout; defaults are 2,000 and 10,000 ms. |
| `--device ID` | `setup`: select a discovered candidate ID. `doctor`: select a configured device alias. |
| `--output FILE`, `--name NAME` | `setup`: configuration destination and optional local display name. No remote rename. |
| `--adb PATH`, `--curl PATH` | `setup`: explicitly select dependency executables. |
| `--server-port PORT` | `setup`: dedicated loopback ADB server port; default 15037. This is not the TV's connection port. |
| `--manual-endpoint IPv4:PORT`, `--manual-kind legacy\|tls` | `setup`: explicitly supply an endpoint on the selected LAN and its protocol. Does not perform pairing. |
| `--confirm` | `setup --json`: allow configuration publication; does not grant TV/tunnel access. |
| `--resume` | `setup`: allow reuse of matching existing configuration; changed files are not overwritten. |
| `--checkpoint FILE`, `--no-checkpoint` | `setup`: select or disable saved choices. JSON mode uses a checkpoint only when explicitly supplied; dry-run never writes one. |

Offline examples, from a built checkout:

```sh
./target/release/tv-control interfaces --fixture examples/discovery.json --json
./target/release/tv-control discover --fixture examples/discovery.json \
  --interface lan-fixture --json
./target/release/tv-control setup --dry-run --json
./target/release/tv-control doctor examples/config.json --json
```

These do not contact a TV. A fixture is simulated data, not evidence of device
access. For live discovery, omit `--fixture` after selecting your LAN interface.
Avoid changing playback just to test configuration.

## Guided connection

`onboard CONFIG` continues from saved setup, asks for access approvals and verifies
the conversation. `onboard CONFIG --json` reports pending without granting access.
`disconnect CONFIG` stops only its exact owned controller and preserves
configuration and TV trust. Use the [onboarding guide](ONBOARDING.md) for the
interactive steps, runtime lifecycle and recovery.

`bridge`, `onboard --launcher`, and `onboard --fixture FILE --json` are integration
entry points used by the installer/runtime and offline tests, not alternate user
setup shortcuts. The simulated journey always reports `ready:false`.

### Doctor version evidence

`doctor CONFIG --json` includes a `versions` object. `invoked_version` identifies
this CLI binary (which is not necessarily the installation's stable launcher).
`last_started_version` is the saved bridge version only when its private record
matches the current generation and exact configuration snapshot.
`last_started_matches_invoked` compares those package versions; it is `null` when
no matching record exists. Same-version development builds are not distinguished.
Older bridges did not record a build version and therefore report unknown.

These local records can outlive their process. `running_version` is deliberately
`null` and `running_verified` is `false`, including with `--probe`: that option
observes the TV via this CLI, not the background bridge. A live MCP `initialize`
response's `serverInfo.version` identifies the responding server. Neither an
installed update nor a historical record proves that the running bridge updated.
Doctor never restarts a bridge, repairs records or grants access. Use the normal
explicit maintenance flow to restart with the installed version.

### Read-only media metadata

`tv-control media-read CONFIG REQUEST.json` (or `-` for stdin) accepts the same
request as MCP `tv_media_read`: configured Jellyfin device, absolute
`expires_at_unix_ms`, and `read` of `search`, `item`, `episodes` or `playback`.
See [media metadata](MCP.md#read-only-jellyfin-media-metadata). Exit status is 0
for an observation, 2 for a failed read, and 1 for invalid CLI/wire input.
This command creates no operation journal, credentials or playback changes.
