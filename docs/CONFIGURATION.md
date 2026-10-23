# Configuration

Start with [examples/config.json](../examples/config.json). It is a complete
version-1 configuration with generic addresses, identifiers and executable paths;
it is not ready to control a real device.

```sh
cp examples/config.json config.local.json
./target/release/tv-control validate config.local.json
./target/release/tv-control capabilities config.local.json
```

Run commands from the repository root after [building](DEVELOPMENT.md).
`*.local.json` is ignored by Git; keep real configuration outside the checkout
where practical. Validation checks structure, resource bounds and references. It
does not check executable availability, credentials, device access or compatibility.
The CLI accepts configuration files up to 1 MiB and rejects unknown fields.

`tv-control setup` can generate a private ADB-only draft without manual JSON.
An optional ADB `discovery_service` field preserves the escaped DNS-SD service
identity for diagnostics; it does not resolve or refresh a dynamic endpoint.
Rediscover after device/debugging changes. See [guided setup](ONBOARDING.md).

## Configuration sections

| Section | Purpose |
| --- | --- |
| `version` | Must be `1` |
| `generic_adb_control` | Optional explicit category grant, default `false` for old configurations |
| `preferences` | Deadlines, polling, transport and memory bounds |
| `executables` | Absolute paths for `adb` and `curl`; both fields are required |
| `devices` | Named ADB or Jellyfin targets; at least one is required |
| `apps` | App aliases mapped to Android package names |
| `servers` | Jellyfin base URLs and environment variable names for tokens |
| `media` | Media aliases mapped to a server and series ID |
| `workflows` | Aliases for a single configured Jellyfin series-resume action |

All sections except `generic_adb_control` are required; maps other than `devices` can be empty when unused. Map aliases must be unique,
1–64 ASCII letters, digits, underscores or hyphens. Limits are 64 devices, apps,
media entries and workflows, and 16 servers. App packages must be unique.

## ADB device

```json
{
  "adapter": "adb",
  "coordination_group": "screen",
  "endpoint": "192.0.2.10:5555",
  "server_host": "127.0.0.1",
  "server_port": 15037,
  "media_app": "player",
  "volume_max_index": null
}
```

This is a `devices` entry, not a complete configuration. `endpoint` is an explicit
numeric IP address and nonzero port already known to the ADB daemon. The server
address must be IPv4 loopback with a nonzero port. There is no discovery, pairing
or connection setup command.

`media_app` references an `apps` alias for the legacy restricted path. Omit it or
use `null` to disable legacy targeted pause/resume. With the explicit generic
grant, the controller instead targets the uniquely observed foreground app/session. App entries contain a `package`, such as `org.example.player`, with
at most 200 characters; each dot-separated component starts with a letter and
contains only ASCII letters, digits or underscores.

`volume_max_index` is optional; `null` disables volume control. An enabled value
must be 1–1000 and match the observed Android music stream maximum on that device.
It does not define a percentage scale. Read the [ADB daemon requirement and
remaining race](ADAPTERS.md#adb-daemon-ownership) before using a real target.

## Jellyfin device and credentials

A Jellyfin device contains `adapter: "jellyfin"`, `coordination_group`, `server`,
`user_id` and `device_id`. The server is an alias in `servers`. User, device and
series IDs must be 1–128 ASCII letters, digits, underscores or hyphens; use exact
identities from your authorized installation.

```json
{
  "base_url": "https://media.example.invalid",
  "token_env": "TV_LIBRARY_TOKEN",
  "allow_insecure_http": false
}
```

This is a `servers` entry. `token_env` names an environment variable in the
controller's environment, not the token itself. Its name may contain only
uppercase ASCII letters, digits and underscores, with a length of 1–128.
Supply the credential through your private process supervisor or client; never
put it in a tracked configuration, command example or issue report.

The base URL may include a path prefix but must have no trailing slash, userinfo,
query, fragment, whitespace or embedded credential. HTTPS is required unless
`allow_insecure_http` is explicitly enabled. TLS validation stays enabled and
redirects are not followed. See [Jellyfin compatibility](ADAPTERS.md#jellyfin)
for the version check and session requirements.

## Media and workflows

A media entry contains a `server` alias and a `series_id`. A workflow contains a
`device` alias and a `media` alias, both on the same Jellyfin server. It expands to
one `resume_media` action using the server's saved resume position. It cannot
contain scripts, shell fragments, URLs or a sequence of remote-control keys.

The public example defines `continue_series` for `media_screen` and `series`:

```sh
./target/release/tv-control workflow config.local.json continue_series request_001 8000
```

This command can start playback. The final argument is a local budget in
milliseconds; it starts when the CLI constructs the request, not when a user
sent a chat message.

## Physical-device coordination

Give every alias for the same screen the same `coordination_group`, including
aliases using different adapters. The example uses `screen` for both ADB and
Jellyfin. Known duplicate targets assigned to different groups are rejected;
cross-protocol identity is not discovered automatically.

Groups prevent overlapping operations only within one running controller.
Separate processes and physical remotes are outside this coordination mechanism.

## Resource policy

Every preference is explicit; there are no installation-specific defaults.

| Preference | Meaning and accepted bound |
| --- | --- |
| `max_budget_ms` | Maximum remaining request budget, 1–10,000 ms |
| `verification_poll_ms` | Wait between verification reads, 1–500 ms |
| `process_poll_ms` | Subprocess and stdio polling interval, 1–50 ms |
| `max_output_bytes` | Bounded subprocess I/O and ordinary serialized MCP text output, 4,096 bytes–1 MiB; the example uses 65,536 bytes. Explicit screenshots alone use a fixed 1,500,000-byte image frame cap (1 MiB PNG maximum) |
| `max_frame_bytes` | Incoming request/frame limit, 1,024–65,536 bytes |
| `max_inflight` | Concurrent MCP control calls, 1–8 |
| `dedup_entries` | In-memory request entries, 1–4,096 |
| `dedup_retention_ms` | Completed receipt retention, at least `max_budget_ms` and at most 86,400,000 ms |
| `resume_position_tolerance_ms` | Allowed seek-position tolerance, 0–10,000 ms, plus elapsed time since dispatch |
| `max_queued_responses` | MCP output queue capacity, 1–64 messages |
| `output_timeout_ms` | MCP write timeout, 1–10,000 ms |

The 4,096-byte minimum reserves space for MCP tool discovery. Output capacity
must also fit configured capability listings; larger configurations may need more
space. Poll intervals do not
create fresh deadlines; the original request budget covers the whole operation.
See [MCP backpressure](MCP.md#transport-bounds) and [request semantics](CLI.md#deadlines-and-retries).

## Generic ADB permission and catalogue

`generic_adb_control` grants opening installed applications, playback, native
search and UI navigation on configured ADB targets, including applications
installed later. The interactive onboarding flow asks for this category grant
before saving it. Updating the binary does not grant it. Rejecting the prompt
leaves the configuration unchanged; old configured aliases remain available.

The discovered catalogue is not configuration or an allowlist. Bounded private
cache data is stored separately in `.tv-control-catalogue` beside the configuration.
Onboarding warms it after ADB access is authorized. Missing names do not invalidate
connection verification; subsequent read requests can continue discovery.
Operation recovery uses `.tv-control-operations` in that same directory. Preserve
that journal when restarting a bridge; deleting it removes duplicate protection.
