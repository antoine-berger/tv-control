# tv-control

A small Rust controller for Android TV and Jellyfin, with a CLI and MCP tools.

## Install

Run in a terminal on macOS Apple Silicon, or Linux ARM64/x86-64:

```sh
curl -fsSL https://raw.githubusercontent.com/antoine-berger/tv-control/latest/install.sh | sh
```

The installer checks its files, selects the TV and guides device access and the
ChatGPT connection. Run the same command to resume. Configuration is preserved;
credentials are never saved in progress files. No flags are needed.

This source describes the v0.3.0 journey under review. The `latest` channel stays
on its previously validated release until review, CI and public asset checks pass.

The `latest` download channel points to a published, validated release and may
serve a prerelease. The script keeps its release version pinned internally.

This command executes downloaded code. [Inspection and checksum verification](docs/INSTALLATION.md)
are available before execution. Maintained Android Platform Tools are required
for ADB. TV authorization and background access need explicit approval. The
official tunnel client and account/workspace permissions are required for hosted
access; browser approvals and plugin selection remain guided steps.

## Scope

ADB supports observed state, app opening, pause/resume and calibrated volume.
Jellyfin supports session control and resuming configured media. Commands carry
a deadline; dispatch alone does not count as a verified result. Cast and YouTube
Music automation are not implemented. The ten-second chat-to-TV target and router
runtime remain unqualified.

The journey reports ready only after a read-only TV call through the selected
conversation and a returned verification code. This cooperative check does not
cryptographically attest conversation identity. Remote tunnel provisioning and
new TLS ADB pairing are not performed automatically. Actual hosted and hardware
use of this integration remains unqualified.

## Documentation

- [Installation and recovery](docs/INSTALLATION.md)
- [Device setup and chat connection](docs/ONBOARDING.md)
- [Configuration](docs/CONFIGURATION.md), [CLI](docs/CLI.md) and [MCP](docs/MCP.md)
- [Adapters and limits](docs/ADAPTERS.md)
- [Development and source builds](docs/DEVELOPMENT.md), [architecture](docs/ADR.md)
- [Router qualification](docs/ROUTER.md), [acceptance](docs/ACCEPTANCE.md) and [measurements](benchmarks/README.md)

[MIT licence](LICENSE).
