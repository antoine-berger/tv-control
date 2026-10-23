# Installation and recovery

## Guided installation

```sh
curl -fsSL https://raw.githubusercontent.com/antoine-berger/tv-control/latest/install.sh | sh
```

The script defaults to the version shipped with that URL. It detects the host
platform: macOS Apple Silicon or Linux ARM64/x86-64. macOS Intel is refused before
download or setup; existing files and historical release assets are preserved.
It asks for confirmation, verifies an existing installation, installs or
updates its own files when needed, then runs the wizard. Repeating the same
command reuses an intact current version without downloading it again. An older
installation is updated after confirmation; previous payloads and external
configuration are retained. A modified or unowned installation is refused.

### Download channel

The `latest` branch points to an existing published release commit. Its script is
identical to that release's installer asset and keeps the asset version pinned.
This channel may distribute validated prereleases; it does not change their status.

Promote the channel only after the release's required CI/review checks pass, its
assets are published and their public downloads/checksums and fixture-based install
flow are verified. Point `latest` to that same commit, then check that the raw URL
returns HTTP 200 and exactly the published script bytes, with the matching internal
version. Keep the versioned releases and their assets unchanged.

Discovery displays a short Searching for TVs message and supports Ctrl-C.
Its default receive window is 2 seconds, with a separate 10-second overall
search timeout. It returns after the short window rather than waiting 10 seconds.
A local display name can be chosen after selection; this changes configuration
metadata only and never renames the remote device.

The wizard selects the only LAN interface when there is one, or asks which LAN
to use. It searches that LAN directly and saves the new local configuration without
an extra confirmation; existing files are never overwritten. TV
advertisements are unverified candidates. Network failure shows the stage and a
numeric OS error when available, then offers manual TV connection details in the
same wizard. The OS cause is not inferred from a generic failure code.

Confirmations and wizard input use `/dev/tty`; the script stream is code only.
The complete installer function is parsed before execution. Without a controlling
terminal the guided flow stops before installation writes. No sudo or shell
profile edits are used. After saving configuration, v0.3.0 continues with explicit
ADB and background-access approvals and [conversation verification](ONBOARDING.md).
Declining those approvals does not create their corresponding access. Remote
tunnel creation and ChatGPT account/plugin approvals remain separate guided steps.

## Resume

Run the same installation command after a discovery failure, cancellation or
restart. Configuration defaults to `$HOME/.config/tv-control/tv-control.local.json`
(or the selected XDG config home). A private adjacent checkpoint remembers the
validated interface, display choice, advertisement identity, local name, explicit
manual endpoint and dependency paths. It contains no credentials or claims of device/tunnel authorization.

The saved interface/address is checked again. Resuming setup repeats the bounded discovery on the selected LAN without an
extra confirmation; snapshots and dynamic ports are not trusted across
processes. A disappeared or changed display requires a fresh selection. Older
checkpoints without advertisement identity also require selection again, while
keeping a saved name when the same address is explicitly chosen. A manual endpoint is
reconfirmed before reuse. An already saved valid private configuration is kept,
without discovery or rewriting it.

An advertisement that expires during user input triggers one bounded refresh in
this wizard on the same interface/address and for the same service identity.
The name and choice stay saved; a current port replaces the expired port in the
new draft. If the selected identity cannot be revalidated, the wizard offers retry
without restarting the installer. No other TV is selected automatically.

Checkpoint writes use a private temporary file, sync and atomic publication. A
process-held file lock is released after process exit. Incomplete initial hard-link
publication has a guided recovery for recognized duplicate temporary links only.
Unreadable private state can be backed up and restarted after confirmation.
Corrupt data is preserved; unsafe ownership, permissions, symlinks and unknown
hard links are refused. Cooperative locking does not protect against a hostile
process running as the same user, and filesystem durability is not guaranteed.

Configuration choices and connection progress have separate private checkpoints.
Neither stores credentials or substitutes for fresh TV/runtime observations.
Maintained Android Platform Tools (ADB) and the official connection helper can be
acquired after separate explicit approvals in the wizard on supported hosts, as below.
Account/plugin approvals and media preferences remain separate configuration work.
See [onboarding](ONBOARDING.md).

## ADB prerequisite recovery

A valid explicitly selected ADB or an executable already on PATH is preserved.
If missing, guided setup offers installation or an absolute executable path, and
can retry in the same session without losing the selected TV. JSON/noninteractive
and dry-run setup never install dependencies. Installation requires explicit
acceptance of the linked [Android SDK license](https://developer.android.com/studio/terms)
and installation approval; it does not authorize an ADB identity or TV connection.

- Linux x86-64: the wizard can install Google's pinned Platform-Tools 37.0.1 ADB
  privately beside configuration under `connection-dependencies`. Its ZIP's embedded
  SHA-256 pin was verified against the Homebrew cask and Google's official HTTPS
  repository metadata (which separately publishes SHA-1). The complete archive's
  twelve exact paths are checked, then only `adb`, `NOTICE.txt` and
  `source.properties` are streamed to fixed private destination names. ELF machine
  checks happen without executing ADB. A receipt verifies subsequent setup reuse.
- This Google build requires a compatible glibc runtime and system libraries.
  Automatic acquisition is offered only when the x86-64 loader
  `/lib64/ld-linux-x86-64.so.2` exists; that check alone does not qualify every host.
  The inspected pinned binary needs glibc symbols through 2.16 and `libgcc_s.so.1`;
  it does not need the ZIP's `lib64/libc++.so`. Pure-musl routers and Linux ARM64
  have no supported Google archive in this flow. Install a maintained native ADB
  through the host's package manager, then retry or enter its absolute path.
- macOS: if Homebrew already exists, the wizard offers
  `brew install android-platform-tools`, using Homebrew's current cask and checksum.
  Without Homebrew, install official Platform-Tools yourself and enter the ADB path
  or retry. No Homebrew bootstrap, sudo, shell-profile edits, security bypass,
  daemon start, pairing or device access occurs during dependency acquisition.

Failed downloads/declined approvals preserve setup choices. A private partial
installation is never published. Existing external ADB files are not overwritten;
explicitly installing or entering a new path selects it for the new configuration.
Private artifacts and configuration remain after controller uninstall.

Sources: [Google release notes](https://developer.android.com/tools/releases/platform-tools),
[Google repository metadata](https://dl.google.com/android/repository/repository2-3.xml),
[Homebrew cask](https://github.com/Homebrew/homebrew-cask/blob/main/Casks/a/android-platform-tools.rb).

## Connection helper

The interactive journey reuses an existing `tunnel-client` on PATH. If missing:

- Linux x86-64/ARM64: with approval, download the official `openai/tunnel-client`
  v0.0.17 ZIP over HTTPS into private staging. SHA-256 pins are embedded in the
  adapter, checked against the upstream release's `SHA256SUMS.txt` when updating
  this code. The archive must contain exactly the seven expected root files.
  Files are streamed individually, never extracted to archive-supplied paths;
  ELF architecture is checked without executing either binary. The client,
  bundled cloudflared, companion manifest, license notices and SBOM are published
  atomically under the private connection directory, with executable mode 0700.
  `curl`, `unzip` and `sha256sum` (or `shasum -a 256`) in `/usr/bin` or `/bin` are required.
- macOS: with separate approval, run `brew install openai/tools/tunnel-client`.
  Homebrew must already exist; the wizard does not install Homebrew. This uses
  Homebrew's prefix and the current official formula/dependencies, not the pinned
  private Linux installation. Automatic Homebrew updates and analytics are disabled
  for this invocation. Upstream's ZIPs are not notarized; the wizard never bypasses
  Gatekeeper or strips quarantine attributes.

No dependency acquisition creates credentials, a tunnel, a background process,
account access or a plugin. Those have their own later approvals. Noninteractive
installer and dry-run modes do not acquire the connection helper.

An interrupted Linux download leaves no published partial installation; rerunning
retries it. An intact private install is checksum-verified and reused without a
new download, including during resume and disconnect. Modified files, foreign
installations, symlinks, hard links and unexpected members fail closed. No existing
PATH binary is overwritten. Configuration and private dependencies are retained by
controller uninstall. Cooperative locks do not protect against a hostile process
running as the same user. Checksums establish integrity relative to the reviewed
upstream release; they are not independent publisher signatures.

If Homebrew is missing/fails, required Linux tools are missing, or acquisition
fails, the checkpoint is preserved. Correct the prerequisite and rerun the same
command. The wizard still needs a supported host/runtime, the listed system tools or package
manager, account permissions and the user's TV/chat approvals to finish setup.

Upstream references: [v0.0.17 release](https://github.com/openai/tunnel-client/releases/tag/v0.0.17),
[checksums](https://github.com/openai/tunnel-client/releases/download/v0.0.17/SHA256SUMS.txt),
[macOS installation](https://github.com/openai/tunnel-client/blob/v0.0.17/README.md#install-with-homebrew).

## Verify before execution

Download `install.sh` and `install.sh.sha256` from the same pinned release, then
verify with `shasum -a 256 -c install.sh.sha256` on macOS or
`sha256sum -c install.sh.sha256` on Linux. Read the script and run `sh install.sh`.
The pipe command executes downloaded code without this inspection opportunity.
Checksums from the same release establish integrity relative to that release;
they are not independent publisher signatures. Binaries are unsigned/not notarized.
The script also verifies each platform archive checksum and ELF/Mach-O architecture.

## Advanced and agent use

The normal flow needs no flags. For agents, `--json` and `--non-interactive` never
prompt or start setup. `--dry-run` does not download, open a terminal or write.
Other options are listed by `sh install.sh --help`; they are intended for explicit
version pinning, alternative dedicated prefixes and offline archives.

An advanced same-version `--setup` reuse checks an explicit `--sha256` pin against
the recorded archive digest. It refuses archive/checksum overrides instead of
silently ignoring them. Pinned v0.2.0–v0.2.2 setup uses the arguments supported by
those binaries; it does not gain the new checkpoint behavior. Noninteractive
updates require an explicit version and `--update`. No moving `latest` URL is used.

Default installation root: `$HOME/.local/share/tv-control`. Payloads are versioned,
private and activated with an atomic current-version file. The installer does not
edit PATH. Existing symlink path components and writable-by-others installation
parents are refused. Uninstall validates the recorded exact contents before removal;
changed files, unexpected entries and special files stop it before deletion.
Configuration and checkpoints are outside the installation and are preserved.
For a v0.3.0 owned managed controller, uninstall verifies and stops that runtime
before deleting payloads. If stopping cannot be verified, installation files are
preserved. Local key removal or uninstall does not revoke the TV's trust record.

Archives are restricted to known regular members, at most 16 MiB compressed and
32 MiB decompressed. Paths, links, duplicate entries and architecture mismatches
are checked before activation. Tests use temporary HOME directories and fixture
network responses; no real TV setup is implied by CI success.
