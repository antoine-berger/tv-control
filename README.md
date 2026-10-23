# tv-control

Control Android TV and Jellyfin from a terminal or an MCP client such as ChatGPT.
The Rust controller checks device state after each action and reports whether the
result was observed, verified or uncertain.

- **Android TV (ADB):** read state, open configured apps, pause/resume and set
  calibrated volume.
- **Jellyfin:** control an active session or resume a configured series.
- **CLI and MCP:** the same actions, explicit deadlines and bounded I/O. No
  free-form shell commands or screen captures.

## Get started

On macOS Apple Silicon or Linux x86-64/ARM64, run:

```sh
curl -fsSL https://raw.githubusercontent.com/antoine-berger/tv-control/latest/install.sh | sh
```

This downloads and executes the installer. To inspect it first, see
[download and checksum verification](docs/INSTALLATION.md#verify-before-execution).

The wizard selects your TV, saves a private configuration, asks you to authorize
TV/background access, and guides the ChatGPT connection. Keep the TV available
for its authorization prompt. Rerun the same command to resume interrupted setup.

You need a reachable Android TV with debugging enabled, maintained ADB, and, for
ChatGPT, the official tunnel client and eligible account/workspace permissions.
The wizard offers approved dependency installation on supported hosts; macOS
requires existing Homebrew for that route. Linux ARM64 and pure-musl hosts need a
maintained native ADB supplied separately. [Requirements and recovery](docs/INSTALLATION.md).

This source targets **v0.3.1**. The `latest` distribution installer advances only
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
