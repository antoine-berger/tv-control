# Installation and recovery

## Before you start

- Use macOS Apple Silicon or Linux x86-64/ARM64. macOS Intel is not supported.
- Put the host and TV on a reachable LAN, enable the TV's supported debugging
  mode and keep its screen available for authorization. A TLS pairing port is not
  a control endpoint; new TLS ADB pairing must be completed separately.
- Keep an interactive terminal open: setup reads approvals from `/dev/tty`.
- For ChatGPT, you need an eligible workspace, an authorized tunnel and a
  restricted runtime key. Enter the key only at the hidden wizard prompt.

## Guided installation

```sh
curl -fsSL https://raw.githubusercontent.com/antoine-berger/tv-control/latest/install.sh | sh
```

This executes downloaded code. To inspect it first, follow
[checksum verification](#verify-before-execution) below.

The installer detects the platform, asks for confirmation, checks its payload and
starts the wizard. It does not use sudo or edit shell profiles. An intact current
installation is reused; an older version is updated after confirmation. Previous
payloads and external configuration are retained. Modified or unowned files are
refused rather than overwritten.

The wizard selects a LAN and TV, offers missing dependencies, and saves private
configuration. Discovery normally listens for two seconds, within a ten-second
overall timeout. Advertisements identify candidates, not authorized devices. If
search fails or a TV has no usable advertisement, the wizard offers manual
connection details. A chosen display name is local metadata only.

Setup then continues through [TV authorization, background access and ChatGPT
verification](ONBOARDING.md). Each access grant needs approval. Remote tunnel
creation, account/workspace approval and plugin selection remain guided external
steps. Installation alone does not establish a working chat-to-TV connection.

### Download channel

The `latest` branch supplies a published release's exact installer, with its asset
version pinned internally. It can distribute a validated prerelease without
changing that release's status. Source on `main` can be newer than the release.
Maintainers: see [publication and promotion](https://github.com/antoine-berger/tv-control/blob/main/docs/RELEASING.md).

## Installed files and useful commands

Default launcher: `$HOME/.local/share/tv-control/tv-control`. It is not added to PATH.
Default configuration: `${XDG_CONFIG_HOME:-$HOME/.config}/tv-control/tv-control.local.json`.

```sh
"$HOME/.local/share/tv-control/tv-control" --help
"$HOME/.local/share/tv-control/tv-control" doctor \
  "${XDG_CONFIG_HOME:-$HOME/.config}/tv-control/tv-control.local.json" --json
```

`doctor` checks local configuration without contacting the TV. Add `--probe` only
when you want a read-only device check. It does not pair or reconnect a device.

## Resume

Rerun the same installer after interruption, cancellation or a dependency failure.
Private checkpoints remember selected interfaces, display identity, local names
and dependency paths. They contain no credentials and do not substitute for TV or
tunnel authorization. A valid saved private configuration is kept without rewriting.

Before using an unfinished selection, setup rechecks its LAN and advertisement
identity. Dynamic ports are rediscovered; a disappeared or changed display needs
fresh selection, and manual endpoints are reconfirmed. An advertisement that
expires during input gets a bounded refresh without losing the name and choice.

Unreadable progress can be backed up and restarted after confirmation. Corrupt
state is preserved. Unsafe ownership, symlinks and unexpected hard links are
refused. Do not bypass those checks to resume. Checkpoints use private atomic
publication and cooperative locking; they are not protection from a hostile
process running as your user.

Configuration and connection progress are separate. A healthy owned controller
can be reused without asking for its key again; changed configuration or a
restarted controller needs fresh validation. See [runtime recovery](ONBOARDING.md#resume-and-stop).

## ADB prerequisite recovery

An explicitly selected valid ADB or one found on PATH is preserved. If ADB is
missing, the wizard offers a supported install or an absolute executable path.
You can retry without losing the selected TV. JSON and dry-run setup never install
it. Installation requires approval and acceptance of the linked
[Android SDK license](https://developer.android.com/studio/terms); it does not
start a daemon, pair a TV or authorize a device.

| Host | Guided acquisition |
| --- | --- |
| macOS Apple Silicon | With existing Homebrew, `brew install android-platform-tools`. Otherwise install official Platform-Tools yourself and supply the ADB path. |
| Linux x86-64 with glibc loader | Pinned official Google Platform-Tools 37.0.1, checksum-verified into private `connection-dependencies` storage beside configuration. |
| Linux ARM64 or pure-musl | No supported Google archive in this flow. Install maintained native ADB through the host's package manager, then retry or enter its absolute path. |

The Linux x86-64 route requires `/lib64/ld-linux-x86-64.so.2`, glibc symbols through
2.16 and `libgcc_s.so.1`. The loader check alone does not qualify every host.
Only allowlisted archive members are read; executable architecture and subsequent
reuse are checked without executing the downloaded binary during acquisition.
Existing external ADB files are not overwritten. The wizard never bootstraps
Homebrew, uses sudo or bypasses macOS security controls.

Sources: [Google release notes](https://developer.android.com/tools/releases/platform-tools),
[repository metadata](https://dl.google.com/android/repository/repository2-3.xml),
[Homebrew cask](https://github.com/Homebrew/homebrew-cask/blob/main/Casks/a/android-platform-tools.rb).

## Connection helper

The interactive journey reuses an existing `tunnel-client` on PATH. If missing:

- **macOS:** with approval and existing Homebrew, install the official
  `openai/tools/tunnel-client` formula. This uses the current formula and its
  dependencies; Homebrew is not bootstrapped. Quarantine/Gatekeeper are not bypassed.
- **Linux x86-64/ARM64:** with approval, download pinned official v0.0.17 into
  private staging, verify embedded SHA-256 pins and the exact archive members,
  check ELF architecture, then publish the client and bundled files atomically.
  This requires `curl`, `unzip` and `sha256sum` (or `shasum -a 256`) in `/usr/bin`
  or `/bin`. Host/runtime compatibility still needs qualification.

A verified private install is reused; partial downloads are not published.
Changed files, unexpected archive members and unsafe links are refused. Existing
PATH binaries are not overwritten. If acquisition fails, correct the prerequisite
and rerun; saved setup choices remain available.

Installing this helper does not create credentials, a remote tunnel, a background
process or a plugin. Those steps have separate approvals. Noninteractive and
dry-run flows do not acquire it. Account/workspace eligibility is still required.

Upstream: [v0.0.17 release](https://github.com/openai/tunnel-client/releases/tag/v0.0.17),
[checksums](https://github.com/openai/tunnel-client/releases/download/v0.0.17/SHA256SUMS.txt),
[macOS installation](https://github.com/openai/tunnel-client/blob/v0.0.17/README.md#install-with-homebrew).

## Verify before execution

From the same [versioned release](https://github.com/antoine-berger/tv-control/releases),
download `install.sh` and `install.sh.sha256` into an empty directory. Read the
script, then verify and run it:

On macOS:

```sh
shasum -a 256 -c install.sh.sha256 && sh install.sh
```

On Linux:

```sh
sha256sum -c install.sh.sha256 && sh install.sh
```

Run the installer only if verification succeeds. The installer also checks its
platform archive's checksum, contents and ELF/Mach-O architecture before activation.
Checksums establish integrity relative to the selected release, not an independent
publisher signature. Binaries are unsigned/not notarized.

## Stop and uninstall

To stop the managed controller while keeping the installation:

```sh
"$HOME/.local/share/tv-control/tv-control" disconnect \
  "${XDG_CONFIG_HOME:-$HOME/.config}/tv-control/tv-control.local.json"
```

To remove it, download and inspect the installer as above, then run:

```sh
sh install.sh --uninstall
```

Uninstall checks the recorded exact file set and, for an owned v0.3.0 managed
controller, verifies that runtime stopped before removing payloads. Modified
files or an unverified stop preserve the installation. Configuration, checkpoints,
private dependencies and TV trust remain. Revoke the trusted computer on the TV
separately when retiring it; deleting local keys does not revoke remote trust.

## Advanced and agent use

`sh install.sh --help` lists version pinning, dedicated prefixes and offline
archive options. `--json` and `--non-interactive` do not prompt or start setup;
`--dry-run` does not download or write. Noninteractive updates require an explicit
version and `--update`.

Same-version `--setup` reuse checks an explicit `--sha256` against its recorded
digest and refuses ignored archive/checksum overrides. Installing an old release
uses that release's supported setup arguments; it does not gain new behavior.

See the [CLI reference](CLI.md#discovery-and-setup) for direct setup, fixtures and
JSON previews. Automated install tests use isolated homes and simulated responses;
they do not establish real TV or hosted-chat acceptance.

## Troubleshooting

| Symptom | Next step |
| --- | --- |
| Installer needs a terminal | Run it from an interactive terminal. Guided setup uses `/dev/tty`; JSON/noninteractive modes do not grant access. |
| No TVs found | Check the selected LAN and the TV's debugging mode. Discovery advertisements are optional and unverified; use the wizard's manual endpoint path if needed. Do not use a TLS pairing port as the control port. |
| Search interrupted or TV port changed | Rerun the same installer. It retains choices and refreshes discovery before using a dynamic endpoint. |
| ADB missing | Accept the supported dependency install, or provide an absolute path to maintained ADB. Linux ARM64/pure-musl hosts need a native package; macOS automatic acquisition needs existing Homebrew. |
| TV says unauthorized | Approve this controller's private identity on the TV. An authorization for your usual ADB daemon does not authorize the controller's separate identity. |
| Helper install failed | Check the [platform prerequisites](#connection-helper), correct the missing tool or package-manager issue, and rerun. Saved TV choices remain available. |
| Local check passes but ChatGPT cannot see tools | Complete tunnel/workspace approval, install and select the plugin in the intended chat, then complete the fresh verification challenge. See [onboarding](ONBOARDING.md). |
| Verification code rejected after a restart | Rerun onboarding and use its new challenge. Proofs expire after 120 seconds and are bound to a controller process generation. |
| Modified files, unsafe ownership or links refused | Preserve the files and inspect the reported path. Do not bypass ownership or checksum checks or delete configuration as a first step. |
| Action returned `outcome_unknown` | The command may have reached the device. Inspect current state before deciding what to do; do not automatically repeat the mutation. |

For a bug report, include the version, host OS/architecture, failing stage and
error code. Remove keys, private addresses, device/user IDs, viewing history and
private paths. [Contributor guidance](DEVELOPMENT.md#public-contributions) lists
what helps reproduce a problem safely.
