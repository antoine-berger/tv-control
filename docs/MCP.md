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
  `local_elapsed_ms` does not measure chat latency. Generic receipts label this
  `timing_scope: application_execution`: application entry through result
  construction for executed operations, excluding final journal persistence and
  response transport. Early rejections may report zero; recovery retains the
  original measurement.
- Generic receipt `state_scope: execution_observation` labels an optional state
  sample from the original execution, including `tv_operation` recovery. It is
  never a fresh read at recovery time. On failure/unknown outcomes, this is the
  last completed in-budget sample, not proof the requested effect occurred.
  A different foreground package does not establish a redirect or launch success.
  Connection/playback may be partial; missing values mean unobserved, not false.
  Media identity/position is excluded from failure diagnostics and package text
  is bounded. A failed, late or cancelled observation cannot replace an earlier
  accepted sample. Pre-dispatch state is cleared before mutation; an uncertain
  dispatch with no post-dispatch sample has no state evidence.
- Diagnostics add no reads or retries. Only the existing persisted `observed`
  field is reused; scope labels are wire-only, preserving journal compatibility.
  Read current state explicitly when needed; do not replay an uncertain action.

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
This attempts insertion at the current cursor/selection, but accepts success only
when a fresh accessibility dump of the same uniquely identified native editor
equals its earlier dumped text plus the requested text. This is UI observation,
not proof of underlying committed editor contents. It does not move the cursor,
clear text, submit a search, or select a result. Selection replacement or insertion away from the end
therefore fails verification. The adapter rejects multiple focused nodes, password
fields, unknown target packages, missing/ambiguous resource identity, unreadable or
truncated text, and a changed foreground package. The total expected dumped text
must fit 256 characters. The UI read exposes bounded `accessibility_text` separately
from display `text` labels, without a content-description fallback. An empty dump
attribute differs from a content description; absent `accessibility_text` means
usable dump text is unavailable, not empty.
It rechecks the hierarchy and TTL before dispatch, then reads accessibility-dump text
and foreground again within the original deadline.

Android `input text` synthesizes virtual keyboard events; it is **not** a direct
text commit. An IME/layout or application can transform even ASCII (for example,
requested `Tanya` was observed as `Tqnyq` on hardware). This check detects mismatching exposed text,
not every underlying corruption, and cannot prevent its first occurrence. There is no universal insertion guarantee and
no keyboard-layout inversion, IME/locale change, helper installation, or automatic
repair. A physical remote can also race validation; ADB provides no atomic
compare-and-type operation.

Text accepts 1–500 bytes of printable ASCII except `%` and `?`; the 256-character result
bound can refuse a smaller request with `text_verification_unsupported` before
mutation. Unicode, controls, percent and question marks are rejected, never transliterated. Shell
metacharacters remain one quoted literal argument. Custom editors without readable
native EditText metadata are unsupported. `text_result_mismatch` reports
`outcome_unknown`: keys may already have changed the field, but matching accessibility-dump append was
not observed. Unreadable post-state, changed context, timeout or cancellation after
dispatch also remain uncertain. Read `tv_read` UI (or explicitly capture) for the
current exposed field; its contents are not added to error codes or result
journals. **Never retry an uncertain text operation under a new request ID.** The
reference is consumed even on uncertainty; the existing request ID only retrieves
its cached result. A successful `observed` receipt means matching accessibility-dump text was read
back at that moment. It is not a `verified` committed-text receipt and does not
prove insertion, submission, search, or absence of app text transformation.

Accessibility limits: Android TextView may expose hint text for an empty editor,
or transformed display text instead of underlying text. The stock dump does not
include the showing-hint flag, so absence of that flag is not proof of actual
editor contents. Explicit `showing-hint-text`/`showingHintText` true metadata, when
supplied, is refused; the controller never guesses a hint from its label. A hint
`Search` changing to `Tanya` produces an uncertain mismatch, not a repair/retry.
The [AOSP dumper](https://android.googlesource.com/platform/frameworks/testing/+/refs/heads/main/uiautomator/library/core-src/com/android/uiautomator/core/AccessibilityNodeInfoDumper.java)
also replaces unsupported Unicode/control characters with `?` (and can drop
unpaired surrogates). Therefore a requested or observed question mark is ambiguous
and is refused, not treated as proof of literal text. Even sentinel-free matching
dumps cannot prove that invisible underlying content or a transformation is absent.
See [TextView accessibility text](https://android.googlesource.com/platform/frameworks/base/+/master/core/java/android/widget/TextView.java).

Platform rationale: AOSP's [input shell implementation](https://android.googlesource.com/platform/frameworks/base/+/master/services/core/java/com/android/server/input/InputShellCommand.java)
uses `KeyCharacterMap.VIRTUAL_KEYBOARD`; the [IME shell interface](https://android.googlesource.com/platform/frameworks/base/+/master/services/core/java/com/android/server/inputmethod/InputMethodManagerService.java)
provides IME management, not a generic `commitText` command. This implementation
uses only existing shell/UI capabilities, not a new IME or accessibility service.

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

### Standby (device sleep, not physical display off)

Use `tv_control` with `action: {"type":"standby"}` and the normal device,
expiry and request ID. The generic ADB grant is required. This uses only Android's
[dedicated KEYCODE_SLEEP (223), introduced in API 20](https://developer.android.com/reference/android/view/KeyEvent#KEYCODE_SLEEP).
It never sends a power toggle, reboot, volume/mute command or fallback. Before
sending, the adapter requires an observed SDK of at least 20 and device `input`
help advertising keyevent syntax; otherwise it returns `standby_unsupported`
with status `unsupported`. Help is read from stdout only: devices emitting usage
solely on stderr conservatively remain unsupported, even if their OS can sleep.
These are platform dispatch capabilities, not proof that vendor firmware honors
sleep. `remote_capabilities` exposes `standby_key_supported` and qualifies the
actual effect as device-dependent. No hardware qualification is implied.

A unique native `mWakefulness=Asleep` observation verifies device standby. Already
asleep is a no-op, including a new request ID. Dreaming, Dozing, unknown, duplicate
or absent wakefulness fields do not establish asleep. A successful key dispatch
alone never verifies standby: unknown evidence or lost network after dispatch
returns `outcome_unknown`; still-awake observations are bounded by the original
deadline. Retrieve/retry the same operation ID to recover its journaled result,
not to replay the key, including after process restart. Expiry and cancellation
apply even to an already-asleep no-op.

`tv_read` with `read: {"type":"power"}` reads only native power evidence, with no
mutation grant, foreground app or media session dependency. Known `state.awake`
values carry `power_scope: "android_device_not_physical_display"`. An Android box
being asleep does **not** prove an attached physical TV/display is off; HDMI/CEC
behavior is outside this capability. Unknown formats omit `awake`. Power dumps
share the bounded 1 MiB internal read; oversized/truncated evidence fails closed.



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

### Remote navigation and syntax diagnostics

Generic ADB authorization is required for all these mutations. `navigate` now also
accepts `menu` (Android key 82) and `settings` (176), in addition to the existing
D-pad/select/back/home allowlist. These keys **may be ignored** by the foreground
app, OS policy, or vendor firmware; a successful dispatch is not proof that a menu
or settings screen opened. No arbitrary keycode, shell, power toggle, sleep, or
new audio action is exposed.

```json
{"request_id":"nav_repeat_1","device":"screen","expires_at_unix_ms":1,"action":{"type":"repeat","direction":"down","count":3}}
{"request_id":"nav_hold_1","device":"screen","expires_at_unix_ms":1,"action":{"type":"long_select"}}
{"device":"screen","expires_at_unix_ms":1,"read":{"type":"remote_capabilities"}}
```

Replace the illustrative expiry with a fresh client-generated deadline. `repeat`
accepts only up/down/left/right and an integer count 1–16. It is shorthand for a
bounded `navigate` sequence, dispatched in a single input command, **not** a held
key or an unbounded auto-repeat. Its durable request identity is that expanded
sequence, so equivalent `navigate` and `repeat` requests with the same ID deduplicate.
No delay or custom hold duration is accepted.

`long_select` injects only DPAD_CENTER with the platform `--longpress` option,
after a fresh read-only `input` help probe confirms that option on a keyevent
usage line. Missing/unrecognized help fails closed without injecting a key. This adapter
reads stdout only: older Android builds (including AOSP Android 8/9) that print
input usage solely to stderr cannot establish this evidence and will not enable
long-select, even if their input binary implements it. A false help flag means
no recognized stdout evidence, not proof that the OS or app lacks support.
The option's duration is determined by Android, not by this tool. Menu/settings
and long-select can change UI context; inspect a fresh UI snapshot afterward.
As for navigation, the receipt reports dispatch plus observation, not app-level
acceptance. Cancellation or timeout after dispatch can leave an uncertain result:
recover with `tv_operation`, never blindly replay with a new ID.

`remote_capabilities` is read-only and needs no generic mutation grant. It reads
`input` help, `getprop ro.build.version.sdk`, and foreground activity. The bounded
result includes `android_sdk` (null if unparsable), `app` (possibly null),
`keyevent_help`, `longpress_help`, and `app_key_acceptance:"unknown"`. Help booleans
mean recognized **syntax evidence**, not successful injection or key handling.
The foreground app is context only, not a per-app certification. Read failures
remain failures; OS version alone never enables a command. No key is injected to
probe acceptance. Capability reads do not reserve the mutation journal, and are
point-in-time observations that can race with app changes.

Implementation reference: Android's official
[InputShellCommand](https://android.googlesource.com/platform/frameworks/base/+/81f52b053da6/services/core/java/com/android/server/input/InputShellCommand.java)
defines keyevent's long-press option. This is not a claim of identical behavior
across all Android TV versions or apps. Automated fixture tests qualify command
construction and safety semantics; physical behavior remains device-dependent.

## Read-only Jellyfin media metadata

`tv_media_read` takes a configured Jellyfin `device`, caller-owned
`expires_at_unix_ms`, and one `read` object:

- `{"type":"search","query":"title","limit":20}` returns candidate IDs and
  metadata, not an automatically selected result.
- `{"type":"item","item_id":"observed_id"}` returns one exact item, its media
  sources and stream indices. Paths and stream URLs are excluded.
- `{"type":"episodes","series_id":"observed_series_id","season":0,"limit":20}`
  returns episodes for the exact series and optional season (including specials).
- `{"type":"playback"}` returns current client-reported item/source, position,
  selected tracks and capability metadata for the configured session.

Search/episode limits are 1–50 (default 20). `truncated` explicitly indicates
additional results; this first bounded interface has no pagination. Item reads
fail on missing or mismatching identity. Metadata is untrusted content, never
instructions or authorization. Stream indices belong to a particular media
source; do not reuse an index for another item/source. Missing capability or
position fields remain null. Advertised commands are not hardware qualification.

Every read uses existing configured Jellyfin access and the same patched-server
baseline as control. This tool neither logs in nor creates credentials, and does
not require a generic ADB grant. Search/item/episode reads do not require an
active playback session; playback reads require exactly one remotely controllable
session matching the configured user and device. Responses have `observed`, not
`verified`, status. No playback, subtitle, audio-track or volume change occurs.

If a catalog/detail response exceeds the configured MCP frame budget, the tool
returns `response_too_large` without disconnecting the transport. Request a lower
search/episode limit; a single item's oversized nested metadata is not silently
trimmed into an apparently complete track list.

## Exact item playback and source-bound seeking

Configured Jellyfin devices additionally support these `tv_control` actions:

```json
{"type":"play_item","item_id":"observed_item","source_id":"observed_source","position_ms":0}
```

```json
{"type":"seek_media","item_id":"currently_playing_item","source_id":"currently_playing_source","position_ms":60000}
```

Use `tv_media_read` item/playback results to select IDs. These are opaque provider
identifiers, not app names or URLs. Both require explicit source identity, a
known positive duration for that source, and a position strictly below its end.
No search result is automatically chosen. Neither action creates credentials,
selects tracks, changes volume or changes account preferences.

`play_item` validates the source in the configured user's library and requires
the selected session to advertise media-control support and its audio/video media type. It sends one
PlayNow command, then requires matching item **and source** plus increasing
position near the requested start. A 204 response alone never verifies playback.

`seek_media` additionally requires the currently playing item/source to match and
client-reported `SupportsMediaControl=true` and `CanSeek=true` immediately before dispatch. Verification retains
the pre-request playing/paused state and checks position within configured
`resume_position_tolerance_ms` (extended by elapsed time for playback). A paused
seek does not send play/resume. A playing seek requires progress; unknown source
or position cannot verify. Client reporting/keyframe precision can limit seeking;
this is not a claim of exact physical frame position.

Requests share the existing per-device durable journal, expiry and cancellation
rules. Changed item/source/position under a reused request ID conflicts. After
possible dispatch, a missing postcondition, expiry or cancellation is
`outcome_unknown`; never automatically replay or roll back. Unsupported client
capabilities fail before mutation. These actions are currently implemented only
by the Jellyfin adapter, not universally across TV apps; no live client/version
qualification is claimed by fixture tests.

### Shared journal upgrade boundary

Existing journals without source identity remain readable after upgrade; absent
source fields are not written as new null keys. Journals containing non-null
`media_source_id` (including Jellyfin observations saved in control receipts)
require the updated reader. New relative/percentage volume intents and track-aware
receipts also require the updated reader once written. Upgrade/restart **all** CLI/bridge processes sharing
that journal before using the new volume or Jellyfin controls. Concurrent old/new readers
and downgrade after writing source-aware receipts are not supported. Do not
remove a journal to work around this: it contains no-replay evidence for uncertain
commands. This change does not migrate credentials or change account access.

### Relative and percentage volume

With the existing explicit generic ADB grant, `tv_control` and CLI `execute` accept:

```json
{"type":"adjust_volume","delta":-2}
{"type":"set_volume_percent","percent":50}
```

`delta` is signed native music-stream index steps; `percent` is an integer 0–100
of the **native index scale**, not acoustic volume or an external receiver level.
The application reads current index and maximum under the shared device journal
reservation. It computes one absolute target, then verifies both index and scale.
Percentage rounding is nearest index with ties upward: `(maximum * percent + 50)
/ 100`. For example, 50 on a maximum of 17 targets index 9. Relative out-of-range
targets are rejected without clamping or a write. Unknown, invalid or changing
scales fail explicitly; configured calibration must still match.

The adapter rechecks the range before a write. An already-satisfied target still
requires a live deadline and cancellation check after observation. The operation
ID identifies the original relative/percentage request: recovery returns the
retained result and never recomputes a delta. Different parameters with the same
ID conflict. Physical remotes and independently configured controllers remain
outside this journal's exclusion. These actions do not identify the physical
audio output or establish silence. They issue no explicit unmute or restoration
command; effects on mute state and the physical audio output are unqualified. They are simulated
contracts until separately qualified on an authorized device/output.

### Relative seeking and source-bound track selection

Configured Jellyfin API devices may expose these additional `tv_control` actions:

```json
{"type":"seek_relative","item_id":"active_item","source_id":"active_source","delta_ms":-1000}
{"type":"select_audio_track","item_id":"active_item","source_id":"active_source","index":1}
{"type":"select_subtitle_track","item_id":"active_item","source_id":"active_source","index":4}
```

Fresh `tv_media_read` playback includes typed `capabilities` for absolute seek,
paused-only `seek_relative`, audio-track and subtitle-track selection. These describe current backend evidence, not user
authorization or universal app support. No API configuration means these controls
are unavailable; there is no ADB, global key, UI or default-track fallback.

Relative seek requires reported **paused** playback, current item/source, a
reported position, a positive source duration and fresh `CanSeek` plus
`SupportsMediaControl`. It floors the reported
position to milliseconds, adds the nonzero signed `delta_ms` once under the device
reservation, and validates the resulting absolute target. Overflow, negative
targets and targets at or beyond duration are refused. The immutable absolute
target is used throughout dispatch and verification, without further delta math.
Allowed delta magnitude is at most 922337203685477 ms; this is an arithmetic bound,
not a claim about usable media duration. Verification requires the same item/source
and paused state, plus a strict displacement in the requested direction from the
initial native position. An unchanged position or movement in the opposite
direction cannot verify. When the requested delta magnitude is no larger than the
configured seek tolerance, the observed position must equal the immutable target
exactly; if client/keyframe precision cannot expose that target, the outcome stays
unknown. Larger deltas allow the configured target tolerance, still excluding
unchanged positions and the wrong direction. Native sub-millisecond fractions are
not by themselves evidence of the requested millisecond displacement. Independently
of the target window, the raw native displacement must be at least
`max(1 ms, abs(delta_ms) - tolerance_ms)`. The comparison uses native ticks before
flooring, with saturating arithmetic. This is conservative: even exact target
readback remains uncertain if a fractional initial position means the actual
displacement is smaller than this minimum, including a +1 ms request that only
crosses the next millisecond boundary. No target recalculation or repair follows.

Playing relative seek returns `unsupported` with
`relative_seek_playing_unverifiable` before dispatch. The current observation
contract has no rate/trajectory bound or correlated seek acknowledgement; natural
playback progress near a target cannot establish that a seek was applied. There is
no assumed 1x speed or implicit pause/resume. The prepared relative plan carries a
paused precondition that the backend rechecks after metadata reads, immediately
before dispatch; a newly playing session is refused before the POST. An external
controller can still race the final read and command because the server exposes no
atomic preconditioned seek endpoint. A successful paused receipt describes
observed displacement and identity/state, not causality or physical frame proof.
This restriction is specific to `seek_relative`; the existing absolute `seek_media`
position-window contract is unchanged and does not establish command causality.

Track selection requires an exact active item/source, one unambiguous stream index
of the requested type in that source, and a freshly advertised client command
`SetAudioStreamIndex` or `SetSubtitleStreamIndex` with media-control support.
Metadata alone never proves this capability. Audio indices are 0–2147483647;
subtitle indices are -1–2147483647, with `-1` an explicit off request. Missing/null
indices remain unknown, not off or a default selection. The adapter sends one
typed JSON command and verifies the requested index, item/source and unchanged
playing/paused state. It does not issue play/resume or alter account preferences.

A 204 acknowledgement is dispatch evidence only. An ignored command, changed
identity/state, cancellation or expiry after possible dispatch is uncertain; no
repair, rollback or implicit retry follows. Receipt observations expose
`audio_index` and `subtitle_index` when reported. Old journal receipts without
these fields remain readable. As with source-aware receipts, all processes sharing
a journal must use the updated reader before writing new track evidence; do not
erase the journal to downgrade. No real Jellyfin/client or audio mutation
qualification is claimed by fixture tests.

### Selected app details (read-only)

After discovering apps, request `tv_read` with
`read: {"type":"app_details","app":"org.example.player"}` and the usual device
and deadline. Use an exact returned ID, or the returned component when an ID has
multiple launchers. Labels and partial names are not accepted here. This read
refreshes discovery and does not cache versions or capabilities. It never launches
an app, searches, opens a URL, or injects input; no generic mutation grant is needed.

The result preserves the discovered `id` and launcher `component`. `version` is
`observed` only when a bounded package dump has one unambiguous active package
record, with numeric `code` and optional `name`. Factory versions under **Hidden
system packages** are not the installed active version. Missing, unfamiliar or
ambiguous evidence yields `version.status: "unknown"`.

`native_search.status` is `handler_resolved` (with its component) only when a fresh
package-restricted ACTION_SEARCH resolution names a component inside that package;
`unsupported` means the platform explicitly returned no activity, and `unknown`
means resolution could not establish either outcome. This does not use a launcher
fallback as search evidence. `runtime_acceptance` always remains `unknown`: a
resolved handler need not process the query. These are sequential observations,
not an atomic guarantee against app updates. Existing search dispatch behavior is
unchanged by this read.

`url_support` is `requires_specific_uri_resolution`. Resolving an arbitrary example
URL cannot prove general URL support; a particular URI must be resolved separately.
Output/transport/deadline failures remain bounded errors, not negative capability
claims. Platform read-command failures can leave individual evidence unknown.

### Explicit device media keys

`tv_control` accepts `action: {"type":"remote_media","command":"pause"}` with
exactly `play`, `pause`, `stop`, `next`, or `previous`. This is a dedicated remote
key, **system-global**: Android chooses the recipient. It does not target the
foreground app or a media session: it may affect a background recipient, start
playback, or do nothing. Prefer `media` with an
explicit discovered session when precise targeting is needed. This action is
never an automatic fallback from session controls, and never retries dispatch.

Success is `observed` / `command_dispatched_state_observed`, not verified playback,
content delivery, or a recipient claim. `active_app` is only foreground diagnostic
evidence, not the recipient; playback remains unknown. A fresh SDK and input-help
probe establish platform syntax eligibility, not app support. Any unexpected
command output or post-dispatch failure is uncertain. The shared private input-injection helper forwards separately quoted arguments
to a fixed shell command and merges device stderr into its bounded capture because
Android can print a rejection on stderr while exiting zero; raw diagnostics are
not exposed as receipts. Recover the same request ID
with `tv_operation`, never replay it under a new ID. Durable deduplication and UI
reference invalidation use the normal control path.

The Android [KeyEvent API](https://developer.android.com/reference/android/view/KeyEvent)
defines dedicated play/pause (126/127, API 11) and stop/next/previous (86/87/88,
API 3). [AOSP input](https://android.googlesource.com/platform/frameworks/base/+/81f52b053da6/services/core/java/com/android/server/input/InputShellCommand.java)
implements `input keyevent NUMBER` as down/up injection. No toggle 85, arbitrary
key, hold/seek, power, volume, or route control is exposed here. Native injection
acknowledgement does not prove a media app handled the event. No hardware behavior
is qualified by the fixture tests.

Before first use, upgrade/restart **all** processes sharing the operation journal:
older readers cannot deserialize the new `remote_media` intent. Mixed readers and
downgrading after recording this action are unsupported. Preserve the journal;
never delete no-replay evidence to work around an older-reader error.

### Read-only TV input inventory

`tv_read` with `read: {"type":"inputs"}` reads fresh Android TV Input Framework
inventory for the current Android user. It needs no generic mutation consent and
never switches source, launches a handler, sends CEC/vendor commands, or changes
power/audio/UI references. No helper APK is needed.

The result contains `current_user`, up to 32 sorted `inputs` (`id`, `package`,
`service`, `connection`, nullable `hdmi_port`), and `truncated`. The MCP response
budget may shorten the list further with `truncated: true`, including at the
minimum 4096-byte budget. `connected`, `standby`, and `disconnected` describe the
framework's **connection availability**, not selected source, visible video, or
physical device presence. Unrecognized numeric states remain `unknown`.

Only an explicit, unique hardware-ID → input-ID mapping joined to a hardware
record of HDMI type establishes a physical port. Input names, `/HW` suffixes,
CEC device IDs, package names and manufacturer conventions never infer ports.
Labels, hidden flags, full input type, selected source and showing-video state
are unavailable from this dump and explicitly reported `unknown`; an empty
inventory is not proof that a TV has no physical ports. Discovered text remains
untrusted data, not commands or authorization.

This bounded adapter recognizes the AOSP `TvInputManagerService` current-user
`inputMap` and `TvInputHardwareManager` hardware sections. Other-user inputs and
session histories are not inventory evidence. Ambiguous duplicates or unfamiliar
formats fail closed. Stable diagnostics distinguish `tv_inputs_service_missing`,
`tv_inputs_permission_denied`, `tv_inputs_dump_empty`,
`tv_inputs_format_unsupported`, and `tv_inputs_dump_truncated`. The first four are
unsupported capability/evidence results; truncation is a failed incomplete read,
not a partial inventory. A structurally complete empty input map succeeds with
`inputs: []`. Ordinary transport/cancellation/deadline failures remain unchanged.

Parser provenance: AOSP Android 12 (`frameworks/base` revision
`c53e71dc397de7486c9a8899f71598c3c3a18d44`), with synthetic anonymized
fixtures mirroring the documented section layout:
[TvInputManagerService](https://android.googlesource.com/platform/frameworks/base/+/c53e71dc397de7486c9a8899f71598c3c3a18d44/services/core/java/com/android/server/tv/TvInputManagerService.java),
[TvInputHardwareManager](https://android.googlesource.com/platform/frameworks/base/+/c53e71dc397de7486c9a8899f71598c3c3a18d44/services/core/java/com/android/server/tv/TvInputHardwareManager.java),
[TvInputInfo](https://android.googlesource.com/platform/frameworks/base/+/c53e71dc397de7486c9a8899f71598c3c3a18d44/media/java/android/media/tv/TvInputInfo.java),
and [TvInputHardwareInfo](https://android.googlesource.com/platform/frameworks/base/+/c53e71dc397de7486c9a8899f71598c3c3a18d44/media/java/android/media/tv/TvInputHardwareInfo.java).
`TvInputInfo.toString()` exposes ID/package/service only, not labels/hidden/type.
Fixtures establish parser/transport contracts, not source-selection or playback
qualification on a physical TV.

The adapter uses a fixed, non-interpolated shell wrapper around `dumpsys tv_input`
to merge its diagnostics and append its numeric exit status. The terminal frame
is validated before parsing; missing, ambiguous or interrupted frames never
produce an inventory. Nonzero status only permits classification of known missing
service/permission diagnostics, never successful inventory. ADB transport errors
remain distinct. This is implementation-specific dump evidence, not a stable
Android SDK API; future/vendor formats can legitimately return unsupported.

Hardware HDMI is the HAL constant `9`, not the public `TvInputInfo.TYPE_HDMI`
value `1007`: see the pinned
[TV input HAL](https://android.googlesource.com/platform/hardware/interfaces/+/4a8246e3757732cb787327c3f8ad5cbacf910d1e/tv/input/1.0/types.hal).
A zero process status alone is insufficient: the pinned
[dumpsys implementation](https://android.googlesource.com/platform/frameworks/native/+/3e846409f064570d19769748a3050882c29fa999/cmds/dumpsys/dumpsys.cpp)
can print a missing-service error to stderr while returning zero. The adapter
therefore classifies the captured body as well as checking the status frame.

### Input injection diagnostic boundary

Wake, standby, navigation, long-select, activation and focused text use the same
private bounded input helper as explicit remote-media keys. Zero-exit stderr
diagnostics are uncertain (`input_dispatch_unconfirmed` for these actions), not
proof of no effect. The command is fixed; validated text is a positional argument,
never shell source. Read-only help probes are unchanged. Diagnostics stop further
postcondition reads, consume applicable UI references and remain durable no-replay
receipts. Silent dispatch still requires the existing action-specific evidence;
it does not prove physical playback, silence or committed underlying text.
