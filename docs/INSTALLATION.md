# Installation and recovery

## Guided installation

```sh
curl -fsSL https://github.com/antoine-berger/tv-control/releases/download/v0.2.3/install.sh | sh
```

The script defaults to the version shipped with that URL. It detects the host
platform, asks for confirmation, verifies an existing installation, installs or
updates its own files when needed, then runs the wizard. Repeating the same
command reuses an intact current version without downloading it again. An older
installation is updated after confirmation; previous payloads and external
configuration are retained. A modified or unowned installation is refused.

The wizard selects the only LAN interface when there is one, or asks which LAN
to use. It asks before querying that LAN and before saving configuration. TV
advertisements are unverified candidates. Network failure shows the stage and a
numeric OS error when available, then offers manual TV connection details in the
same wizard. The OS cause is not inferred from a generic failure code.

Confirmations and wizard input use `/dev/tty`; the script stream is code only.
The complete installer function is parsed before execution. Without a controlling
terminal the guided flow stops before installation writes. No sudo, shell profile
edits, daemon, pairing, credentials, service or tunnel is created.

## Resume

Run the same installation command after a discovery failure, cancellation or
restart. Configuration defaults to `$HOME/.config/tv-control/tv-control.local.json`
(or the selected XDG config home). A private adjacent checkpoint remembers the
validated interface, display choice, explicit manual endpoint and dependency
paths. It contains no credentials or claims of device/tunnel authorization.

The saved interface/address is checked again. Discovery is repeated only after a
new explicit confirmation; snapshots and dynamic ports are not trusted across
processes. A disappeared display offers a fresh selection. A manual endpoint is
reconfirmed before reuse. An already saved valid private configuration is kept,
without discovery or rewriting it.

Checkpoint writes use a private temporary file, sync and atomic publication. A
process-held file lock is released after process exit. Incomplete initial hard-link
publication has a guided recovery for recognized duplicate temporary links only.
Unreadable private state can be backed up and restarted after confirmation.
Corrupt data is preserved; unsafe ownership, permissions, symlinks and unknown
hard links are refused. Cooperative locking does not protect against a hostile
process running as the same user, and filesystem durability is not guaranteed.

These checkpoints cover local TV configuration only. TV authorization, maintained
ADB installation, media preferences, tunnel credentials and ChatGPT plugin approval
remain separately verified steps described in [onboarding](ONBOARDING.md).

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

Archives are restricted to known regular members, at most 16 MiB compressed and
32 MiB decompressed. Paths, links, duplicate entries and architecture mismatches
are checked before activation. Tests use temporary HOME directories and fixture
network responses; no real TV setup is implied by CI success.
