# CLI reference

Examples assume a release build and commands run from the repository root.
Use `./target/release/tv-control --help` for the command summary or `--version`
for the package version. Every other command loads and validates its configuration.

## Commands

```text
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
| Open app | `{"type":"open_app","app":"player"}` | Configured app is in the foreground |
| Set volume | `{"type":"set_volume","index":5}` | Observed calibrated Android music stream index matches |
| Resume media | `{"type":"resume_media","media":"series"}` | Matching item, expected seek range and two observations with increasing position |

Action availability depends on the [adapter](ADAPTERS.md). App and media arguments
are aliases, not package names, titles or URLs. There is no free-form shell tool.

## Receipts and exit codes

A receipt contains `request_id`, `status`, `code`, `cached`, `local_elapsed_ms`,
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

Request IDs deduplicate the same device/action within one running process and the
configured retention window. A completed request replays its receipt with
`cached: true`; that evidence is historical. An in-flight duplicate returns busy.
Reusing an ID for a different device or action is invalid. Retained and active
entries are never evicted to admit a new request; a full store returns busy.

Each CLI invocation creates a new store. A persistent MCP process retains its
store until exit, retention expiry or a crash. Restarts lose all entries, and
coordination does not cover other processes or physical remotes. There is no
durable journal or cross-process lease.

Cancellation before dispatch returns `cancelled`. After possible dispatch,
cancellation or timeout returns `outcome_unknown`. Process cleanup cannot undo a
TV command or stop a player that buffers before starting. A lost response after
dispatch is also uncertain for the caller. Do not automatically replay an
uncertain mutation, including after a restart; inspect current state first.

## Guided onboarding

`interfaces`, `discover`, `setup`, `doctor` and `connect-chatgpt` provide the guided
setup and agent JSON interface. See [the complete onboarding reference](ONBOARDING.md)
for options, dry runs, authorization boundaries and offline examples.
