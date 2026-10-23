# Development

Build on Linux or macOS Apple Silicon with a stable Rust toolchain and native linker. Python 3
is used for repository checks and stdio integration fixtures. CI tests native Linux
and macOS Apple Silicon builds and cross-builds the ARM64 musl candidate; it does not run on a
router or TV. Build a source checkout with:

```sh
git clone https://github.com/antoine-berger/tv-control.git
cd tv-control
cargo build --locked --release
./target/release/tv-control --help
```


## Architecture

```text
CLI / MCP stdio -> Control port -> application service
                                      |
                 device | catalogue | state | clock | cancellation ports
                                      |
                   ADB | Jellyfin/curl | memory | system adapters

Dependency direction: domain <- application <- adapters <- CLI composition
```

| Crate | Responsibility |
| --- | --- |
| `tv-domain` | Actions, requests, observed evidence and receipts; no dependencies |
| `tv-application` | Use cases, deadlines, postconditions and ports; depends only on the domain |
| `tv-adapters` | Configuration, serialization, ADB, curl, bounded subprocesses/stdio and memory state |
| `tv-control` | CLI composition and MCP request handling |

The application has no JSON, environment, filesystem, ADB or HTTP knowledge.
`scripts/check_architecture.py` enforces the domain/application dependency boundary.
Direct third-party dependencies are Serde, serde_json and libc; `Cargo.lock` pins
the dependency graph. The binary embeds no model, browser, Rust async runtime or
HTTP/TLS stack. ADB and curl remain external runtime dependencies for their
respective adapters.

## Checks

Run from the repository root:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release
python3 scripts/check_architecture.py
python3 -W error::ResourceWarning scripts/test_installer.py
python3 -W error::ResourceWarning scripts/test_mcp.py target/release/tv-control
python3 -W error::ResourceWarning scripts/test_onboarding.py target/release/tv-control
python3 -W error::ResourceWarning scripts/test_install_flow.py target/release/tv-control
python3 -W error::ResourceWarning scripts/test_zero_arg_ux.py target/release/tv-control
./target/release/tv-control validate examples/config.json
python3 scripts/audit_publication.py
```

The automated suite uses fake clocks, injected ports, fake executables and loopback
HTTP/ADB fixtures. It does not require a real TV, Jellyfin account, pairing or a
router installation. Fixture success does not qualify physical playback.
The integrated install flow uses a temporary HOME and a native test launcher that
passes wire fixtures to the real Rust wizard. It exercises the zero-argument
script stream and controlling PTY together. An optional second binary argument to
`test_install_flow.py` qualifies compatibility with an independently verified
v0.2.2 executable; CI does not silently download or execute old releases.

Subprocess invocations own Unix process groups and bounded nonblocking pipes.
Cleanup terminates owned processes on cancellation, timeout and exit. Inspect
ownership and deadline behavior when changing these adapters. Executables are
trusted configuration; process groups are not a security boundary.

## Connection dependency checks

The dependency adapter tests use injected subprocess fixtures. They cover explicit
approval, platform selection, checksums, unexpected archive paths, executable
architecture, private permissions, foreign/symlink destinations, modified-file
rejection and checksum-verified resume. No fixture invokes tunnel-client or Homebrew.

An optional contract check uses an already downloaded official Linux ZIP, verifies
its embedded release pin, streams its payload into an automatically removed private
test directory and rechecks the receipt. It never executes a downloaded binary:

```sh
TV_CONTROL_TEST_TUNNEL_ARCHIVE=/absolute/path/to/tunnel-client-v0.0.17-linux-amd64.zip \
  cargo test --locked -p tv-adapters --lib official_archive_offline_contract -- --ignored
```

Supply the ARM64 tunnel ZIP instead on an ARM64 Linux host. The corresponding
Google ADB archive check is available on Linux x86-64:

```sh
TV_CONTROL_TEST_ADB_ARCHIVE=/absolute/path/to/platform-tools_r37.0.1-linux.zip \
  cargo test --locked -p tv-adapters --lib official_adb_archive_offline_contract -- --ignored
```

This also only inspects/extracts the fixture and never invokes ADB. Ordinary CI does not download
or install external helpers. macOS Homebrew execution remains an explicitly
approved host acceptance check; injected command tests are not that qualification.

## Changing behavior

Add domain vocabulary only when needed, express behavior through application
ports, then implement and test adapters. Preserve the distinction between failure
before dispatch and an uncertain outcome after possible mutation. Acknowledged
dispatch must not become success without its defined postcondition.

New backends need capability reporting, explicit identity matching, bounded I/O
and contract tests. New configuration fields need validation and generic examples.
Keep workflow configuration declarative; it currently expands to one configured
series-resume intent. See the [architecture decisions](ADR.md) for the rationale
and [review notes](REVIEW.md) for regressions that must stay covered.

## Measurement and qualification

Use the [benchmark procedure](../benchmarks/README.md) for controller-only host
measurements and the [acceptance protocol](ACCEPTANCE.md) for hardware/integration
qualification. Record the source commit and environment with every result. Do not
reuse old host figures as evidence for a new build, an ADB daemon, a tunnel or
router firmware. [PLAN.md](../PLAN.md) tracks the remaining qualification scope.

Router packaging has a separate [cross-build recipe](ROUTER.md). No test or
packaging command installs a service, configures a network or grants TV access.

## Public contributions

Keep real endpoints, credentials, user/device IDs, media history, captures and
private filesystem paths out of commits and issue reports. Public examples use
reserved documentation addresses and fictitious identifiers. The publication
scanner is a baseline check, not a complete secret detector; also review staged
content and untracked files before publishing.

For bug reports, include the source commit, platform, adapter/client versions,
sanitized configuration shape and result codes. Describe expected versus
observed behavior and whether dispatch may have occurred. Never replay an uncertain
mutation just to reproduce a log.
