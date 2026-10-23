# tv-control

Control Android TV and Jellyfin from a terminal or an MCP client such as ChatGPT.
The Rust controller checks device state after each action and reports whether the
result was observed, verified or uncertain.

- **Android TV (ADB):** read state, open configured apps, pause/resume and set
  calibrated volume.
- **Jellyfin:** control an active session or resume a configured series.
- **CLI and MCP:** the same actions, explicit deadlines and bounded I/O. No
  free-form shell commands or screen captures by default.

## Get started

On macOS Apple Silicon or Linux x86-64/ARM64, run:

```sh
curl -fsSL https://raw.githubusercontent.com/antoine-berger/tv-control/latest/install.sh | sh
```

This downloads and executes the installer. To inspect it first, see
[download and checksum verification](docs/INSTALLATION.md#verify-before-execution).

The wizard selects your TV, saves a private configuration, asks you to authorize
TV/background access, and guides the ChatGPT connection. Keep the TV available
for its authorization prompt. Rerun the same command to update or resume: saved
configuration is retained, and maintenance does not repeat device selection.
Runtime-key storage is a separate opt-in private-file choice; older releases did
not save that key. See [update and key recovery](docs/ONBOARDING.md#resume-and-stop).

You need a reachable Android TV with debugging enabled, maintained ADB, and, for
ChatGPT, the official tunnel client and eligible account/workspace permissions.
The wizard offers approved dependency installation on supported hosts; macOS
requires existing Homebrew for that route. Linux ARM64 and pure-musl hosts need a
maintained native ADB supplied separately. [Requirements and recovery](docs/INSTALLATION.md).

This source targets **v0.4.1**. The `latest` distribution installer advances only
when the matching public release has passed download and package verification.
See [releases](https://github.com/antoine-berger/tv-control/releases) for published assets.

## After installation

The installer does not change your PATH. Use its stable launcher:

```sh
"$HOME/.local/share/tv-control/tv-control" --version
"$HOME/.local/share/tv-control/tv-control" doctor \
  "${XDG_CONFIG_HOME:-$HOME/.config}/tv-control/tv-control.local.json" --json
```

`doctor` is offline unless you add `--probe`. For device commands, first review
[configuration](docs/CONFIGURATION.md) and the [CLI examples](docs/CLI.md).
For ChatGPT, finish the [conversation verification step](docs/ONBOARDING.md): a
local health check alone does not establish that the selected chat can use the TV.

## Current limits

TV approval, tunnel/account permissions and plugin selection remain interactive.
New TLS ADB pairing and remote tunnel creation are not automatic. Cast and
YouTube Music automation are not implemented.

Automated checks cover simulated devices and installer flows. Actual TV playback,
the hosted chat-to-TV journey, router deployment and a ten-second end-to-end target
remain unqualified. A verified receipt describes adapter evidence, not proof of
physical picture or sound. See [adapter limits](docs/ADAPTERS.md) and
[acceptance checks](docs/ACCEPTANCE.md).

## Documentation

- **Use it:** [installation](docs/INSTALLATION.md), [onboarding](docs/ONBOARDING.md),
  [troubleshooting](docs/INSTALLATION.md#troubleshooting)
- **Configure it:** [configuration](docs/CONFIGURATION.md), [CLI](docs/CLI.md),
  [MCP integration](docs/MCP.md)
- **Contribute:** [build and test](docs/DEVELOPMENT.md), [source map](https://github.com/antoine-berger/tv-control/tree/main/crates#readme),
  [architecture decisions](docs/ADR.md)
- **Reference:** [all documentation](https://github.com/antoine-berger/tv-control/tree/main/docs#readme), [measurements](benchmarks/README.md)

[MIT license](LICENSE).

### Generic ADB control

Google TV and Android-based Fire OS devices can use discovered applications,
without a configuration alias per app. Other Fire TV operating systems (including
Vega) are not supported by this adapter. Run the normal installer/onboarding
flow to grant the explicit category permission for opening installed apps,
playback, search, and navigation. The grant also covers apps installed later.
Old configurations default to `generic_adb_control: false`; upgrading alone does
not broaden their permissions. Existing configured aliases continue to work.
Declining the onboarding prompt leaves the configuration unchanged.

- `tv_read` returns compact state, a filtered application catalogue, or structured
  UI. App labels are observed, not guessed; use the returned ID if no label is
  available. Catalogue revision queries can return `unchanged` instead of the
  full catalogue.
- `tv_control` accepts `open_app`, `pause`, `resume`, bounded `navigate`, supported
  native `search`, or `activate` with a fresh UI reference. App ambiguity returns
  choices instead of selecting a similar application. UI text is untrusted data,
  never instructions to the agent.
- `tv_operation` recovers a durable result by `request_id`, or requests
  cancellation. Cancellation is not rollback. Never retry an uncertain mutation
  under a fresh ID: the TV may already have changed.

Requests use one absolute client expiry throughout discovery, execution, and
verification. `local_elapsed_ms` measures controller runtime, not request-to-TV
latency. A navigation or search dispatch does not establish that a requested
movie/episode is playing. Native search is available only for apps which expose
an appropriate Android search activity; arbitrary content playback is not
promised. Screenshots are an explicit read-only fallback only; ordinary commands never
request them. Captures are limited to 1 MiB PNG and use a separate bounded
1,500,000-byte image frame allowance; ordinary text budgets remain unchanged.
Protected surfaces may be blank. No companion application is required.

Operation state is private, bounded, and persisted beside the configuration in
`.tv-control-operations`. Direct MCP and the background bridge share this journal
and resource exclusion. Legacy configured intents keep their wire format;
`tv_read`/generic intents and legacy mutations use the same exclusion boundary.
Real-device compatibility and end-to-end timing must be qualified on the target
Google TV/Fire OS hardware; fixture tests do not establish universal support.

App names come from observed metadata or bounded APK resource parsing, never a
hardcoded app registry. Resource extraction currently needs the device's `unzip`
command; unsupported split/overlay resources, locales or missing extraction remain
explicitly incomplete, with exact package/component selection available. Cold
name hydration is bounded and prioritized; warm cached labels avoid extracting
resources again. These fixtures do not qualify Google TV or Fire OS hardware.
