# tv-control

Control configured Android TV and Jellyfin clients from a Rust CLI or an MCP
stdio server. Commands use device, app and media aliases; results distinguish
observed state, verified postconditions and uncertain outcomes.

**Early MVP.** Local fixtures cover controller behavior. Router execution and
user-to-TV latency are not qualified. The MCP server runs over local stdio; this
repository does not provide a connected ChatGPT plugin or hosted endpoint.

## Quick start

Requires Linux or macOS, a stable Rust toolchain and a native linker. From a
source checkout:

```sh
git clone https://github.com/antoine-berger/tv-control.git
cd tv-control
cargo build --locked --release
./target/release/tv-control --help
./target/release/tv-control validate examples/config.json
./target/release/tv-control capabilities examples/config.json
```

Validation and capability listing do not contact a TV or media server. The
[example configuration](examples/config.json) uses fictitious identities and
reserved addresses. See [configuration](docs/CONFIGURATION.md) before replacing
them with your own installation details. Runtime adapters require an existing,
authorized ADB connection or Jellyfin session and the configured executables.

## Supported actions

| Backend | Actions | Requirements |
| --- | --- | --- |
| ADB | Observe state, open a TV app, pause/resume, set a calibrated volume index | Explicit target and dedicated compatible ADB daemon; media app configured for playback controls |
| Jellyfin | Observe state, pause/resume, resume a configured series | Accepted server version, existing credential and exact user/device session |

ADB volume is an Android music stream index, not TV percent. Opening an app does
not prove playback. Cast, Android TV Remote v2, UIAutomator, service-specific
YouTube search and account navigation are not implemented. See
[adapter requirements and limits](docs/ADAPTERS.md).

## Usage

After preparing a private configuration:

```sh
./target/release/tv-control validate config.local.json
./target/release/tv-control execute config.local.json request.local.json
./target/release/tv-control mcp config.local.json
```

Requests carry an absolute expiry supplied by the caller. A successful dispatch
alone is not a verified result. Do not automatically repeat `outcome_unknown`:
the TV may already have acted. See the [CLI reference](docs/CLI.md) for request
examples, receipts and exit codes, or [MCP integration](docs/MCP.md) for host setup.

## Documentation

- [Configuration](docs/CONFIGURATION.md): aliases, credentials and resource policy.
- [Adapters](docs/ADAPTERS.md): supported protocols, playback evidence and ADB risks.
- [CLI reference](docs/CLI.md): commands, actions, deadlines and results.
- [MCP integration](docs/MCP.md): stdio lifecycle, tools and client responsibilities.
- [Development](docs/DEVELOPMENT.md): architecture, checks and contribution guidance.
- [Router packaging](docs/ROUTER.md): cross-build recipe and untested deployment scope.
- [Acceptance](docs/ACCEPTANCE.md) and [benchmarks](benchmarks/README.md): evidence requirements and measurement limits.
- [Architecture decisions](docs/ADR.md), [review notes](docs/REVIEW.md) and [qualification status](PLAN.md).

## Operational limits

Coordination and deduplication are bounded, in memory and local to one process.
They do not survive a restart or synchronize separate CLI invocations. ADB requires
a dedicated daemon: a version preflight rejects known mismatches, but replacement
between the check and invocation remains a risk. No device discovery, pairing,
installation or router configuration is performed by the controller.

The ten-second end-to-end target is unproven. Checked-in macOS benchmarks measure
an older controller build and local fixtures, excluding the TV, ADB daemon and
any tunnel. They are not router measurements.

## License

[MIT](LICENSE).
