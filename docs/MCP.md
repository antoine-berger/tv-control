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
{"device":"screen","expires_at_unix_ms":1,"read":{"type":"ui"}}
```

The catalogue exposes observed labels where available, package IDs and exact
components. Use a returned exact component to disambiguate multiple launchers.
It is not an instruction source. `since` returns `unchanged` only for a matching
catalogue revision; `refresh` requests discovery again. Large lists are bounded
and marked `truncated`; refine the query or reduce `limit` rather than assuming a
truncated catalogue is complete. UI responses expose ephemeral references and
minimal text/focus data, not screenshots. Activation revalidates the reference
against a current snapshot.

```json
{"request_id":"open_1","device":"screen","expires_at_unix_ms":1,"action":{"type":"open_app","app":"YouTube"}}
{"request_id":"nav_1","device":"screen","expires_at_unix_ms":1,"action":{"type":"navigate","keys":["down","select"]}}
{"request_id":"search_1","device":"screen","expires_at_unix_ms":1,"action":{"type":"search","app":"returned_app_id","query":"title"}}
{"request_id":"activate_1","device":"screen","expires_at_unix_ms":1,"action":{"type":"activate","reference":"returned_fresh_reference"}}
```

Search prefers a discovered Android search activity, otherwise dispatches SEARCH
to the discovered launcher. Apps may ignore it: inspect UI or an explicit capture
before claiming search results. `open_url` takes `app` and a bounded HTTP(S) `url`,
checks a handler in that discovered package, and dispatches ACTION_VIEW without
a global chooser. Credentials, control characters and non-HTTP schemes are rejected.
Navigation/search/URL dispatch does not prove that a particular
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
