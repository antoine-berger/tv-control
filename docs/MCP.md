# MCP integration

`tv-control mcp CONFIG` serves two tools over local stdio. It has no network
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
| `tv_control` | Request object from the [CLI reference](CLI.md#requests) | Execute one configured intent with an absolute expiry |

The server's `tools/list` response is the input-schema reference. Tool results
contain a JSON receipt in both text content and `structuredContent`. `isError` is
true for control receipts other than `observed` and `verified`. Transport or
argument errors use JSON-RPC errors instead of successful control receipts.

The JSON-RPC message ID identifies the call and its cancellation notification.
The nested `request_id` identifies the operation for process-local deduplication;
these identifiers have different roles.

## Client responsibilities

- Stamp the absolute expiry at user submission and preserve it through model,
  transport and queue time. The server does not renew it on arrival.
- Keep one controller process for shared in-memory coordination. Separate
  processes cannot share reservations or duplicate receipts.
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
plain `mcp CONFIG` server keeps its two existing tools. Verification accepts only
the fresh challenge issued by that installation's wizard, with no caller-selected
path, device or action. It performs Observe against the bound configuration and
returns a code only for an uncached, connected observation before expiry.

The user returns that code from the chosen conversation to the wizard. A local
receipt alone is insufficient. Codes are ephemeral and single-use; cancellation
before or after publication suppresses the code and removes the pending proof.
The wizard rechecks runtime health and device access before Ready. This cooperative
workflow does not cryptographically identify a chat and does not grant credentials
or modify TV playback. See [the guided journey](ONBOARDING.md).
