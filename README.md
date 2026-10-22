# tv-control

A small Rust controller for Android TV and Jellyfin, with a CLI and two MCP tools.

## Install

Run in a terminal on macOS or Linux (ARM64 or x86-64):

```sh
curl -fsSL https://github.com/antoine-berger/tv-control/releases/download/v0.2.3/install.sh | sh
```

The installer asks for confirmation, checks an existing installation and opens the
setup wizard. Run the same command again to resume after an interruption. Local
setup progress is saved privately; a changed network or display is checked again.
Existing configuration is preserved. No version, output path or setup flags are
needed for this flow.

This command executes downloaded code. [Inspection and checksum verification](docs/INSTALLATION.md)
are available before execution. Maintained Android Platform Tools are required
for ADB. Device debugging authorization and the optional ChatGPT connection need
separate explicit approvals; the installer does not grant them.

## Scope

ADB supports observed state, app opening, pause/resume and calibrated volume.
Jellyfin supports session control and resuming configured media. Commands carry
a deadline; dispatch alone does not count as a verified result. Cast and YouTube
Music automation are not implemented. The ten-second chat-to-TV target and router
runtime remain unqualified.

The resumable wizard currently prepares local TV configuration. It does not resume
or automate TV pairing, credentials, tunnel provisioning or ChatGPT plugin approval.

## Documentation

- [Installation and recovery](docs/INSTALLATION.md)
- [Device setup and chat connection](docs/ONBOARDING.md)
- [Configuration](docs/CONFIGURATION.md), [CLI](docs/CLI.md) and [MCP](docs/MCP.md)
- [Adapters and limits](docs/ADAPTERS.md)
- [Development and source builds](docs/DEVELOPMENT.md), [architecture](docs/ADR.md)
- [Router qualification](docs/ROUTER.md), [acceptance](docs/ACCEPTANCE.md) and [measurements](benchmarks/README.md)

[MIT licence](LICENSE).
