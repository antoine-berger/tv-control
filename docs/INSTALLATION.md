# User-scoped shell installer

The `v0.2.1` patch provides the installer, its SHA-256 file and four platform
archives. It leaves the existing `v0.2.0` assets unchanged. Releases are pinned;
there is no request to a moving `latest` endpoint.

## Review before execution

The preferred path is to download a pinned `install.sh` and its matching SHA-256
file into a new directory, verify them with `shasum -a 256 -c install.sh.sha256`
(macOS) or `sha256sum -c install.sh.sha256` (Linux), read the script, then execute
it with `sh`. The release will provide both files alongside the four archives.
The installer itself checks the archive checksum before parsing or publishing it.
Checksums from the same GitHub release protect integrity relative to that release;
they are not an independent signature or protection against publisher compromise.
An independently obtained archive hash can be pinned with `--sha256 HEX`.

```sh
mkdir tv-control-installer-v0.2.1
cd tv-control-installer-v0.2.1
curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --output install.sh https://github.com/antoine-berger/tv-control/releases/download/v0.2.1/install.sh
curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --output install.sh.sha256 https://github.com/antoine-berger/tv-control/releases/download/v0.2.1/install.sh.sha256
shasum -a 256 -c install.sh.sha256
less install.sh
sh install.sh --version v0.2.1 --setup
```

On Linux use `sha256sum -c install.sh.sha256` if `shasum` is unavailable.
Run each step only after the previous one succeeds. `--setup` explicitly starts
the guided CLI after interactive installation confirmation. It is incompatible
with `--json` and `--non-interactive`; agents cannot implicitly start discovery.
For an unattended installation use `sh install.sh --version v0.2.1 --json`.


Downloading then executing leaves a review opportunity. A `curl | sh` one-liner
executes downloaded code before inspection and trusts the same distribution
channel; it is not the recommended default. The installer requires explicit
`--non-interactive` when stdin is a pipe. It never invokes sudo or edits shell
startup files. Tests install only into disposable temporary prefixes; existing user installations
are not touched.

## Layout and activation

The default root is `$HOME/.local/share/tv-control`. Use `--prefix ABSOLUTE_DIR`
for a different **dedicated** root. Existing symlink path components are refused.
The immediate parent must be user-owned and not writable by other users/groups.
Use a canonical private path if a home or mount location is a symlink.

```text
tv-control/
  .tv-control-owner
  current
  tv-control                 # small launcher
  versions/v0.2.1/
    tv-control               # verified release binary
    .receipt
    .manifest
    ... bundled docs/examples
```

The root and version directories are private. Payload files are mode 0600;
executables are 0700. Initial publication moves a complete staged root into
place. Updates add a version without overwriting the old payload, then atomically
replace the small `current` file. Old versions remain available. A cooperative
lock serializes installer operations. This is not a security boundary against a
hostile process running as the same user, nor a filesystem durability guarantee.

```sh
sh install.sh --non-interactive --version v0.2.1
sh install.sh --non-interactive --version v0.2.1 --update
sh install.sh --non-interactive --uninstall
```

Updates require `--update` and an explicitly specified version; `latest` is never
resolved. An existing unowned directory or changed installation is refused. A
stored version with a different archive digest is a conflict. Repeating an update
to an identical verified version is safe. The launcher directory can be added to
PATH manually, or invoke it by its full path; the installer does not create a
symlink in an unrelated bin directory or silently modify PATH.

Uninstall validates all stored payload hashes and the exact known file/directory
set before removing recorded files. Unexpected files, changed payloads, symlinks or special entries (including FIFOs
and sockets) cause refusal before any deletion. It never recursively removes an installation root or user config.
An interrupted uninstall can leave partial owned files for manual inspection.
Do not put personal data inside the installation root. User configuration belongs
outside it and is preserved.

## Guided and agent modes

In a terminal, installation asks for confirmation and then offers guided setup.
Only an explicit `y` starts the CLI wizard, which may query the LAN interface the
user chooses. Its config defaults to
`${XDG_CONFIG_HOME:-$HOME/.config}/tv-control/tv-control.local.json`.
ADB dependencies and device authorization remain separate, as do the official
Secure MCP Tunnel credentials and plugin approvals. No daemon, key, pairing,
authentication, service or tunnel is started by the installer.

`--non-interactive` and `--json` never launch setup or read prompts. JSON mode
writes a single structured result to stdout; diagnostics use stderr. `--dry-run`
does not download, create directories, install or start setup. The mode is suited
to an agent that needs a concrete plan/result without implicit device access.
`--archive FILE --checksum FILE` supports explicit offline or air-gapped input.
The checksum filename must identify the expected platform asset. These files are
untrusted input and receive the same archive validation as HTTPS downloads.

The installer also reads the binary header without executing it and refuses an
ELF/Mach-O format or architecture mismatch before activation. Filesystem errors
produce one structured failure in JSON mode when stdout remains available; the
previous active version is preserved before activation. Existing controller
processes are not restarted or stopped; their running version remains unchanged.

The script supports POSIX sh on macOS/Linux, ARM64 and x86-64. It needs standard
Unix utilities (including od), tar, gzip, mktemp, a SHA-256 provider (`sha256sum`, `shasum` or
OpenSSL), and curl for HTTPS downloads. Dependencies are not installed silently.
Archives are limited to 16 MiB compressed and at most 32 MiB decompressed. Parsing
rejects unknown/absolute/traversal names, duplicate members, links and special
files. Each allowlisted member is streamed to a file created in a private stage;
archive paths, modes and ownership are never extracted into the filesystem.

See [guided setup](ONBOARDING.md) for capability evidence and authorization limits.
The router runtime, real discovery, TV effects and ten-second chat-to-TV goal
remain separately unqualified.
