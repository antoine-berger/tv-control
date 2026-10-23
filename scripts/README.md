# Repository scripts

Run scripts from the repository root with Python 3. The [development checklist](../docs/DEVELOPMENT.md#checks)
is the primary test entry point; [CI](../.github/workflows/ci.yml) is the complete
platform/release matrix.

| Group | Scripts | Purpose |
| --- | --- | --- |
| Static checks | `check_architecture.py`, `audit_publication.py` | Inner-crate dependencies and tracked-content baseline audit |
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
