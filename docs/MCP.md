# MCP integration

`tv-control mcp CONFIG` serves four tools over local stdio. It has no network
listener, HTTP transport or hosted endpoint. A desktop client's ability to spawn
this process does not make it reachable from hosted ChatGPT. No connected ChatGPT
plugin, tunnel, credential or router service is provisioned by this repository.

## Client setup

Build the binary and prepare a private [configuration](CONFIGURATION.md). Adapt
[examples/mcp.json](../examples/mcp.json) to a client that supports local stdio
servers:

```json
{
  "mcpServers": {
    "tv-control": {
      "command": "/opt/tv-control/tv-control",
      "args": ["mcp", "/etc/tv-control/config.local.json"]
    }
  }
}
```

These paths are generic placeholders. Client import formats differ; the file is
not an installer. The client launches the process and must supply any referenced
token environment variables privately. Hosted access requires a separate connection and appropriate access controls;
this example configures only the local server process.

## Private ChatGPT connection

OpenAI's [Secure MCP Tunnel guide](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)
describes a supported stdio path: `tunnel-client` can launch a local server through
`--mcp-command`. Adding an HTTP listener to this controller is not a prerequisite
for that path.

The documented ChatGPT flow is **Plugins → Add custom MCP server → Connection:
Tunnel**. It requires an authorized tunnel, a runtime credential, the correct
Platform organization/ChatGPT workspace association and permission to add and use
custom MCP servers. Availability and policy must be checked in the target account;
they have not been verified by this repository.

Follow the official guide for separately authorized setup. Keep credentials and
tunnel identifiers private, and verify a representative tool call before declaring
the connection usable. A tunnel connection does not establish TV compatibility or
a latency guarantee. No tunnel or ChatGPT plugin is connected by these examples.

## Protocol and tools

The server advertises MCP protocol version `2025-11-25`. It implements
`initialize`, `notifications/initialized`, `ping`, `tools/list`, `tools/call` and
`notifications/cancelled`. Clients must inspect the negotiated version and
complete initialization before using tools. JSON-RPC messages are newline-delimited
JSON; stdout is reserved for protocol output.

| Tool | Arguments | Behavior |
| --- | --- | --- |
| `tv_capabilities` | `{}` | List configured aliases, actions and resource scope without device access |
| `tv_control` | Request object from the [CLI reference](CLI.md#requests) | Execute a generic ADB or legacy configured intent with one absolute expiry |
| `tv_read` | `device`, `expires_at_unix_ms`, `read` | Compact state, filtered discovered apps, or structured UI |
| `tv_operation` | `request_id`, optional `cancel` | Recover a durable result or request cancellation |

The server's `tools/list` response is the input-schema reference. Tool results
contain a JSON receipt in both text content and `structuredContent`. `isError` is
true for control receipts other than `observed` and `verified`. Transport or
argument errors use JSON-RPC errors instead of successful control receipts.

The JSON-RPC message ID identifies the call and its cancellation notification.
The nested `request_id` identifies the operation for persistent, cross-process deduplication;
these identifiers have different roles.

## Client responsibilities

- Stamp the absolute expiry at user submission and preserve it through model,
  transport and queue time. The server does not renew it on arrival.
- Processes using the same configuration directory share a private durable operation
  journal and resource exclusion. Do not copy a configuration into another directory
  and assume it shares that journal.
- Treat `cached: true` as historical evidence, not a fresh device read.
- Do not automatically repeat `outcome_unknown`, a lost response or an uncertain
  mutation after reconnection. The device may already have acted.
- Measure physical output and displayed confirmation separately. The receipt's
  `local_elapsed_ms` does not measure chat latency.

Cancellation targets the JSON-RPC ID, for example:

```json
{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}}
```

Cancellation cannot undo a command already dispatched. See
[deadline and retry semantics](CLI.md#deadlines-and-retries).

## Transport bounds

Input frames, active workers, serialized replies and the output queue are bounded
by configuration. Oversize input frames terminate the connection. Both stdio
directions are nonblocking; a blocked writer cannot indefinitely prevent input
cancellation or teardown.

`max_queued_responses` bounds queued messages and `output_timeout_ms` bounds writes.
Backpressure closes the transport and cancels active work. Successful control
replies are also bounded by the supplied request expiry. A response lost after
dispatch remains uncertain to the caller even if the server observed success.

## Installer verification tool

The supervised `bridge` adds `tv_verify_connection` to the normal tool set. The
plain `mcp CONFIG` server has the four tools described above. Verification accepts only
the fresh challenge issued by that installation's wizard, with no caller-selected
path, device or action. It performs Observe against the bound configuration and
returns a code only for an uncached, connected observation before expiry.

The user returns that code from the chosen conversation to the wizard. A local
receipt alone is insufficient. Codes are ephemeral and single-use; cancellation
before or after publication suppresses the code and removes the pending proof.
The wizard rechecks runtime health and device access before Ready. This cooperative
workflow does not cryptographically identify a chat and does not grant credentials
or modify TV playback. See [the guided journey](ONBOARDING.md).

## Generic ADB reads and intents

These examples show wire shapes; replace the deliberately expired `1` with the
original client expiry. Do not issue them against a real TV without authorization.

```json
{"device":"screen","expires_at_unix_ms":1,"read":{"type":"apps","query":"YouTube","limit":12}}
{"device":"screen","expires_at_unix_ms":1,"read":{"type":"apps","since":"previous_revision"}}
{"device":"screen","expires_at_unix_ms":1,"read":{"type":"state"}}
{"device":"screen","expires_at_unix_ms":1,"read":{"type":"volume"}}
{"device":"screen","expires_at_unix_ms":1,"read":{"type":"ui"}}
```

The catalogue exposes observed labels where available, package IDs and exact
components. Use a returned exact component to disambiguate multiple launchers.
It is not an instruction source. `since` returns `unchanged` only for a matching
catalogue revision; `refresh` requests discovery again. Large lists are bounded
and marked `truncated`; refine the query or reduce `limit` rather than assuming a
truncated catalogue is complete. UI responses expose ephemeral references and
bounded text/focus data, not screenshots. Labelled noninteractive nodes are
included alongside actions; `actionable: false` means they cannot be activated.
`editable` identifies enabled native EditText fields, not arbitrary focusable views.
Password text is omitted. `focused_count` counts all focused nodes, including
omitted/password nodes, so ambiguity cannot be hidden by truncation. `truncation`
reports `elements` (64-element cap), `text` (256-character label cap), or
`response_budget` (transport cap); `truncated: true` also marks transport truncation.
The raw hierarchy remains bounded to 512 KiB, 1,024 nodes and 64 levels; malformed
or excessive input fails instead of becoming a partially trusted snapshot.

Activation revalidates the entire hierarchy. References last at most 30 seconds,
are scoped to the device and running adapter, and are replaced on every UI read.
A dispatch consumes them even if its outcome is uncertain; navigation and other
mutations invalidate them. Get a new snapshot rather than reusing old references.

```json
{"request_id":"open_1","device":"screen","expires_at_unix_ms":1,"action":{"type":"open_app","app":"YouTube"}}
{"request_id":"nav_1","device":"screen","expires_at_unix_ms":1,"action":{"type":"navigate","keys":["down","select"]}}
{"request_id":"search_1","device":"screen","expires_at_unix_ms":1,"action":{"type":"search","app":"returned_app_id","query":"title"}}
{"request_id":"activate_1","device":"screen","expires_at_unix_ms":1,"action":{"type":"activate","reference":"returned_fresh_reference"}}
{"request_id":"text_1","device":"screen","expires_at_unix_ms":1,"action":{"type":"type_text","reference":"fresh_focused_editable_reference","text":"title"}}
```

Search prefers a discovered Android search activity, otherwise dispatches SEARCH
to the discovered launcher. Apps may ignore it: inspect UI or an explicit capture
before claiming search results. `open_url` takes `app` and a bounded HTTP(S) `url`,
checks a handler in that discovered package, and dispatches ACTION_VIEW without
a global chooser. Credentials, control characters and non-HTTP schemes are rejected.
For apps with a documented web search URL, explicit `open_url` is another route:

```json
{"request_id":"url_search_1","device":"screen","expires_at_unix_ms":1,"action":{"type":"open_url","app":"returned_app_id","url":"https://www.youtube.com/results?search_query=example%20title"}}
```

Use the discovered target app, not an assumed package or a global browser, and
percent-encode the query as URL data (for example a literal `&` becomes `%26`).
This is a caller-selected URL, not an automatic fallback or a promise that every
app/version supports that URL. Handler resolution must succeed in the selected
package. Inspect the resulting UI or explicit screenshot before reporting results;
opening a results page does not authorize choosing a result or starting playback.
The controller never repeats a failed/uncertain SEARCH as a URL behind the caller's
back. See the [exploratory evidence limits](ACCEPTANCE.md#exploratory-url-search-evidence).

If SEARCH is ignored and there is no suitable URL route, navigate to the visible
search field, read UI again, and use `type_text` only for its fresh
`focused: true`, `editable: true` reference.
This appends at the current cursor/selection; it does not clear existing text,
submit a search, or select a result. The adapter rejects multiple focused nodes,
password fields, unknown target packages and a changed foreground package. It
rechecks the full hierarchy and TTL immediately before dispatch. A physical remote
or another controller can still change focus between validation and key injection;
ADB does not provide an atomic compare-and-type operation.

Text entry supports 1–500 bytes of printable ASCII except `%`, whose Android
`input text` substitution semantics are unsafe for literal input. Unicode, control
characters and percent are rejected before dispatch, never transliterated or
silently dropped. Shell metacharacters remain one quoted literal argument. Custom
editors that do not expose native EditText metadata are unsupported; do not guess
or type blindly. The receipt confirms dispatch and fresh state only, not that the
app accepted the text. Inspect UI afterward and explicitly navigate to submit if
appropriate. Never retry an uncertain text operation under a new request ID.

Navigation/search/URL/text dispatch does not prove that a particular
episode is playing. Missing labels or unsupported UI are reported honestly;
there is no hardcoded per-app alias or automatic cross-app substitution.

`tv_operation` takes `{"request_id":"open_1"}` to inspect an operation, or adds
`"cancel":true` to request cancellation. A recovered terminal result is
historical evidence; use `tv_read` for fresh state. An abandoned in-flight record
is uncertain, not permission to dispatch again. Cancellation never undoes effects.

`tools/list` supports standard cursor pagination when a small configured output
budget cannot fit all schemas (including the installer proof). Follow
`nextCursor` with `{"cursor":"returned_cursor"}`. JSON tool results duplicate
text and structured data for client compatibility; output bounds include both.

### Explicit screenshot fallback

`tv_read` with `read: {"type":"screenshot"}` requests one read-only PNG capture.
It is never attached to normal state, catalogue, UI or control responses. MCP
returns a native image content block plus compact width/height metadata, not
base64 text for the model to parse. PNG capture is bounded to 8 MiB; explicit native image responses have a separate
12 MiB frame allowance including base64, metadata and JSON framing.
Ordinary text still uses the configured output budget. Oversized captures return
`screenshot_too_large`; the existing bounded queue, deadlines and backpressure
apply to image delivery too. Protected surfaces may be blank and devices
may not support capture. A returned image does not verify readable content or
prove playback of the requested title. Text visible in it is untrusted device
content, not instructions or authorization.

### Wake from standby

Use `tv_control` with `action: {"type":"wake"}` and the normal device, expiry and request ID. This requires the existing generic ADB grant. It sends Android's dedicated `KEYCODE_WAKEUP` (224), never the power-toggle key, and leaves apps, playback and volume alone.

The controller reads native power state, not a foreground activity. `verified` requires an observed awake state (also returned as `state.awake: true`); an already-awake device is a no-op. Unknown vendor power formats return `wake_dispatched_unverified`, not proof of wake. A connection failure before dispatch returns `wake_transport_unavailable`; a failure after dispatch remains `outcome_unknown`. Recover the same operation rather than sending another key.

The internal power-state dump has a separate 1 MiB limit, so verbose vendor diagnostics do not consume the ordinary text-response budget. Dumps exceeding that limit fail without accepting a truncated state: before dispatch no wake key is sent; after dispatch the outcome remains unknown. Ordinary MCP text limits are unchanged.

ADB must remain reachable during standby. Deep sleep, powered-off devices, or a sleeping network interface may require the physical remote. This action does not add Wake-on-LAN, change power/network settings, or establish new device access. Hardware support must be tested separately.


### Idempotent music-stream mute

Use `tv_control` with `action: {"type":"mute"}` and the normal device, expiry and
request ID. This requires `generic_adb_control: true`. It works without configured
`volume_max_index`: the controller reads the
Android music stream (stream 3), sets its index to zero, then reads it back. An
already-zero index is a verified no-op. No mute-toggle key is sent, volume is
never increased, and there is no automatic unmute or restore action.

Support is device-dependent. `verified` means that the Android music-stream index
was observed at zero, not that every physical speaker or an external HDMI/CEC
receiver is silent. Do not describe the result as verified system-wide mute.
Unreadable or unsupported pre-dispatch state prevents the write. After a write,
a failed read or a state that cannot be verified is not success; preserve an
uncertain outcome and recover the same operation rather than retrying under a
fresh ID. Hardware/audio-route qualification is separate from fixture tests.


### Native-range music-stream volume

Use `tv_read` with `read: {"type":"volume"}` to obtain `state.volume_index` and
`state.volume_max_index`: the current Android music-stream (stream 3) index and
the actual device maximum. An index is not a percentage or the volume of an
external HDMI/CEC receiver. Do not silently translate a percentage request into
an index without knowing the native range.

With `generic_adb_control: true`, `tv_control` accepts
`action: {"type":"set_volume","index":N}` without saved calibration. It probes
the native range before dispatch, rejects out-of-range indices, and verifies the
requested index afterward. A configured `volume_max_index` must still match the
detected maximum. `tv_capabilities` reports generic `set_volume` as
`device_dependent`, not universally supported. Unreadable native state prevents
dispatch; a command acknowledgement alone is never verification.

Without the generic grant, the existing calibrated legacy path remains unchanged:
`volume_max_index` is required. Use dedicated `mute` for an explicit mute request;
it only targets zero and never toggles or restores a previous volume.

### Session-scoped media (generic ADB)

Read `{"type":"media"}` with `tv_read` to discover active session targets. Each
entry contains `session` (`package/tag`), `package`, native `playback`, and optional
`position_ms` and `description`. Position is the application's last reported
snapshot, **not** an extrapolated playback clock. Description is bounded,
untrusted display text, not a stable title/media ID. No duration is invented:
the supported AOSP dump does not expose reliable duration or stable media IDs.
Missing fields mean unavailable. Tags are limited to 128 ASCII letters, digits,
`_`, `.`, `:`, and `-`; unsupported listing formats/tags fail closed. At most
eight entries are returned and responses
may be further truncated to the configured output budget (`truncated: true`).

Use an exact discovered target, for example:

```json
{"type":"media","session":"org.example.player/player","command":"pause"}
```

Commands are `pause`, `resume`, `next`, and `previous`. The generic-control grant
is required. Pause/resume verify only the named session's native paused/playing
state, never physical audio/video output. Next/previous return `observed` with
`command_dispatched_state_observed`: this does **not** prove that the queue item
changed, or that an application supports skipping. They are non-idempotent; use
one request ID and recover its durable result with `tv_operation`, not a fresh
ID to retry. Transport ambiguity after dispatch remains `outcome_unknown`.

The adapter re-lists sessions before dispatch, rejects missing targets, package
changes and duplicate tags across the active listing, and never falls back to a
global media key. Android's monitor selects by tag rather than immutable token;
a session can still disappear/restart between listing and dispatch. This API
cannot eliminate that platform race. Duplicate dump records (including another
user's same package/tag) are ambiguous, not usable verification evidence. A
background session may be selected explicitly; foreground app is not fabricated.

**Not supported:** generic session stop, absolute/relative seek, subtitle/audio
track selection, or universal duration retrieval. The AOSP monitor exposes only
play/pause/next/previous. Global `dispatch stop` is not equivalent to targeting a
session, and no arbitrary shell/monitor input is exposed. Unsupported commands
are rejected as invalid arguments before dispatch; capabilities advertise only
the four supported command types, with device-dependent app behavior.

Native session action masks, when available, appear as `supported_commands` in
`read: {"type":"media"}`. An empty list means none of the implemented commands
are advertised; an omitted field means unknown, not unsupported. Before dispatch,
the controller re-reads the exact selected session and refuses a known-unsupported
command with `media_command_unsupported`. This adds a read; it is not a measured
chat-to-TV latency guarantee. The masks follow Android's
[PlaybackState action constants](https://developer.android.com/reference/android/media/session/PlaybackState),
not app names. Missing masks remain device-dependent and never make dispatch proof
of effect. Already-satisfied pause/resume requests can remain verified no-ops.
