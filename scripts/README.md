# Repository scripts

Run scripts from the repository root with Python 3. The [development checklist](../docs/DEVELOPMENT.md#checks)
is the primary test entry point; [CI](../.github/workflows/ci.yml) is the complete
platform/release matrix.

| Group | Scripts | Purpose |
| --- | --- | --- |
| Static checks | `check_architecture.py`, `check_source_size.py`, `test_source_size.py`, `audit_publication.py` | Inner-crate dependencies, tracked-source size policy and its fixtures, tracked-content baseline audit |
| CLI/MCP | `test_mcp.py`, `test_onboarding.py`, `test_zero_arg_ux.py` | Protocol, setup and terminal behavior |
| Optional interoperability | `test_tunnel_mcp.py` | Checksum-verified official helper against a loopback protocol fixture; explicit archive required |
| Full journey | `test_install_flow.py`, `test_journey_fixture.py`, `test_complete_journey.py` | Installer/wizard and simulated runtime composition |
| Installer/release tests | `test_installer.py`, `test_release.py`, `test_verify_public_release.py` | Isolated lifecycle and release validation |
| Packaging | `cross_build.py`, `package.py`, `check_package.py`, `check_elf.py` | Cross-builds and allowlisted archives |
| Publication | `release.py`, `verify_public_release.py` | Release assembly/publication and public asset checks |
| Measurement | `benchmark.py` | Controller-only measurements |

`test_*.py` suites use temporary directories and fixtures. Tests that take a
binary path need a preceding `cargo build --locked --release`. Publication and
public-verification scripts have different network/side-effect scopes: read
[releasing](../docs/RELEASING.md) before running them. Cross-builds can acquire
build tools; they are not ordinary fixture tests.

Package membership is explicit in `package.py` and enforced by installer/checker
contracts. New documentation does not automatically enter release archives.
When editing packaged docs, keep local links within the packaged set or use an
absolute repository link for source-only material.

The generic MCP contract is covered by Rust `generic_mcp` wire tests and
`generic_consent_contract`, alongside the existing stdio integration suite.
`test_mcp.py` verifies the expanded four-tool listing, including the minimum
4096-byte transport-output configuration. The cooperative installer proof remains
a separate bridge-only tool. No tests authorize real device control.

`python3 -W error::ResourceWarning scripts/test_generic_mcp.py target/release/tv-control`
exercises the real MCP binary with a loopback ADB version server and a private
fake ADB subprocess. It covers exact-name launch, durable restart replay,
unsupported search and actual generated 1080p/4K PNG frames through the dedicated
image budget, including oversize failure without closing the transport. It never
contacts a real TV and is not a hardware latency qualification.
