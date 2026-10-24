# Development

Build on Linux or macOS Apple Silicon with a stable Rust toolchain and native linker. Python 3.11+
is used for repository checks and stdio integration fixtures. CI tests native Linux
and macOS Apple Silicon builds and cross-builds Linux x86-64 and ARM64 musl candidates; it does not run on a
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

## Find the right code

Start with the [crate map](https://github.com/antoine-berger/tv-control/tree/main/crates#readme).
Each crate README identifies its entry points and tests. For common changes:

- Control actions and result semantics: domain types, application `Service`, then
  the ADB/Jellyfin adapter and its contract tests.
- TV discovery and saved setup: adapter `setup`/`discovery` and CLI `onboarding`.
- Managed connection lifecycle: application `pipeline`/`onboarding`, adapter
  runtime/tunnel/proof modules, then CLI `journey`.
- MCP tools and wire format: CLI `mcp`, adapter `wire` and `scripts/test_mcp.py`.
- Installation/release: `install.sh`, `scripts/package.py`, `scripts/release.py`
  and `.github/workflows/ci.yml`. Package membership is deliberately allowlisted.

Keep a change scoped to its owner; avoid adding I/O or serialization to the inner
crates. Update the user-facing reference when a command or configuration changes.

## Source quality and review policy

Follow the [official Rust Style Guide](https://doc.rust-lang.org/style-guide/)
with `cargo fmt`; use the existing strict
[Clippy check](https://doc.rust-lang.org/clippy/usage.html) rather than inventing
another style engine. Organize related responsibilities into cohesive modules,
keeping implementation details private as described in the
[Rust Book's module guidance](https://doc.rust-lang.org/book/ch07-02-defining-modules-to-control-scope-and-privacy.html).
Splitting files should clarify ownership, not introduce arbitrary indirection or
unnecessary public APIs.

**Project preference, not an official Rust standard:** every maintained Rust or
Python source file must have **fewer than 1,000 physical lines** (999 maximum).
`scripts/check_source_size.py` checks Git-tracked `.rs` and `.py` files, including
tests and fixtures, against the working-tree contents. Blank lines and comments
count; LF and CRLF each end one line, and an unterminated final line also counts.
Stage newly added sources before running the check locally. Missing, non-regular
or non-UTF-8 tracked source files fail the check rather than disappearing from it.

Generated/build/vendor artifacts are not maintained sources: directories named
`generated`, `vendor`, `target`, `dist`, `.venv` and `__pycache__` are excluded.
Lockfiles and non-source artifacts are excluded by extension. A filename such as
`generated.rs` is not an exemption. Do not move maintained code into an artifact
directory to bypass the gate; generators and hand-maintained fixture code remain
subject to the limit. There are currently no per-file size exceptions. Any new
exclusion needs explicit review of its provenance, regeneration procedure and
reason; a maintained-source exception would require a reviewed policy and checker
change, not a suppression comment. Prefer a cohesive refactor before the limit.

The production domain/application core must remain app- and TV-agnostic. Keep
protocol parsing, app-specific behavior and device discovery in adapters behind
ports. Documented protocol constants (for example Android action bits) are valid;
model/package guesses, personal device IDs, screen coordinates or account-specific
fallbacks are not substitutes for discovered/configured capabilities. Fixture
constants are legitimate test inputs, not production device support. Review this
semantically: a grep ban on literals cannot establish agnosticism. Any justified
adapter-specific exception needs a documented protocol/source or configuration
contract, bounded unsupported behavior and focused tests; it must not leak into
the inner crates. The architecture gate checks dependency direction, not all of
these semantic review requirements.

## Checks

Run from the repository root:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release
python3 scripts/check_architecture.py
python3 -W error::ResourceWarning scripts/test_source_size.py
python3 scripts/check_source_size.py
python3 -W error::ResourceWarning scripts/test_release.py
python3 -W error::ResourceWarning scripts/test_verify_public_release.py
python3 -W error::ResourceWarning scripts/test_installer.py
python3 -W error::ResourceWarning scripts/test_mcp.py target/release/tv-control
python3 -W error::ResourceWarning scripts/test_generic_mcp.py target/release/tv-control
python3 -W error::ResourceWarning scripts/test_advanced_mcp.py target/release/tv-control
python3 -W error::ResourceWarning scripts/test_media_read.py target/release/tv-control
python3 -W error::ResourceWarning scripts/test_onboarding.py target/release/tv-control
python3 -W error::ResourceWarning scripts/test_install_flow.py target/release/tv-control
python3 -W error::ResourceWarning scripts/test_zero_arg_ux.py target/release/tv-control
python3 -W error::ResourceWarning scripts/test_journey_fixture.py target/release/tv-control
python3 -W error::ResourceWarning scripts/test_complete_journey.py target/release/tv-control
./target/release/tv-control validate examples/config.json
python3 scripts/audit_publication.py
```

For installer changes, also run `sh -n install.sh`, `shellcheck -s sh install.sh`
and, where available, `busybox sh -n install.sh`, as CI does. The workflow also
checks cross-built archives; the commands above cover native development.

The automated suite uses fake clocks, injected ports, fake executables and loopback
HTTP/ADB fixtures. It does not require a real TV, Jellyfin account, pairing or a
router installation. Fixture success does not qualify physical playback.
The integrated install flow uses a temporary HOME and a native test launcher that
passes wire fixtures to the real Rust wizard. It exercises the zero-argument
script stream and controlling PTY together. An optional second binary argument to
`test_install_flow.py` qualifies compatibility with an independently verified
v0.2.2 executable; CI does not silently download or execute old releases.

Journey teardown requests shutdown through its private fixture directory and
waits for the fake supervisor to acknowledge bridge cleanup. Runtime-state PIDs
are historical records, not ownership handles: never signal them during cleanup.
Only directly owned `Popen` children may be escalated, after waiting/reaping;
permission errors remain failures. A leaked ADB listener still fails the test,
even when the fixture-only fallback subsequently releases it. This avoids stale
PID reuse and Darwin's zombie-only process-group signal behavior (see Apple's
[`killpg1` implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sig.c)).

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

### Optional official tunnel-client protocol qualification

Standard tests keep using isolated fixtures and never download external helpers.
For additional Linux x86-64 or ARM64 interoperability coverage, supply an already
downloaded official v0.0.17 ZIP for the current architecture:

```sh
python3 -W error::ResourceWarning scripts/test_tunnel_mcp.py \
  target/release/tv-control /absolute/path/to/tunnel-client-v0.0.17-linux-amd64.zip
```

Unlike the archive-inspection check above, this opt-in check executes the helper
after verifying its pinned archive checksum and member list. It uses a temporary
HOME, a dummy credential, a loopback-only control-plane fixture and the real
controller's stdio MCP server. It checks repeated initialization, fresh initialized
notifications, malformed-request recovery, the official notification guard and
legacy recovery after an unsupported modern discovery probe.
Only protocol discovery is exercised; no TV tool, ADB, curl, account, real tunnel
or Cloudflare connection is used. All fixture processes are stopped afterward.
This establishes local protocol interoperability, not live ChatGPT or TV success.

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

### Failure classification

`Failure` carries a private semantic kind, separate from its stable diagnostic
code. Use `unsupported(code)`, `expired(code)` or `cancelled(code)` for classified
refusals; `before(code)` is operational by default. Both control paths use the
central `Failure::status()` mapping, so adding a capability refusal needs no
application code-name table. A code ending in `_unsupported` is not itself a
classification (for example an operational failure describing an unsupported
HTTP method or device state retains its existing behavior).

For compatibility, only the exact historical `before("unsupported")`,
`before("expired")` and `before("cancelled")` spellings retain their classifications.
New code should use the explicit constructors. Keep this compatibility set fixed;
do not add suffix rules. `after_dispatch()` preserves the code, classification and
numeric OS evidence while making `OutcomeUnknown` authoritative. The kind is not
serialized: wire codes and status vocabulary and durable journal shapes are
unchanged. Existing journal statuses remain authoritative on replay; loading an
old receipt does not reinterpret its code through the new classification.

The domain type is an internal workspace API, not the MCP wire contract. Its new
private kind intentionally prevents external struct literals: workspace consumers
already use constructors. Any out-of-tree Rust consumer constructing a `Failure`
literal must migrate to the appropriate constructor, followed by
`with_os_error(...)` and, if needed, `after_dispatch()`. Existing public diagnostic
and effect fields remain readable; `FailureKind` is not exported as a new API.
