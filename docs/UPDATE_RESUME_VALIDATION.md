# Update/resume validation (v0.4.1)

Validated on Linux x86-64 on 2026-10-23 using the v0.4.1 source and release build.
No user computer, TV, real credentials or production tunnel was accessed.

## Results

- Rust workspace: **269 passed, 0 failed, 2 ignored** (official dependency archives not supplied).
- Python suites: **188 tests, 187 passed, 1 skipped** (separate verified v0.2.2 executable not supplied).
- Formatting, Clippy with warnings denied, architecture boundaries, tracked-content
  publication audit, shell syntax and whitespace checks passed.
- Independent review covered credential storage, maintenance/restart, installer
  forwarding and first-time verification resumption with no outstanding blocker.

| Python suite | Tests |
| --- | ---: |
| `test_release.py` | 13 |
| `test_verify_public_release.py` | 18 |
| `test_installer.py` | 28 |
| `test_mcp.py` | 15 |
| `test_generic_mcp.py` | 4 |
| `test_onboarding.py` | 29 |
| `test_install_flow.py` | 12 (1 skipped) |
| `test_zero_arg_ux.py` | 12 |
| `test_journey_fixture.py` | 7 |
| `test_complete_journey.py` | 50 |

Coverage includes rerunning the same installer, retaining configuration and ADB
identity, optional device discovery, healthy old-runtime preservation when no key
is available, cancellation, restart generation checks, verification invalidation,
and incomplete first-install resumption without repeating completed onboarding.
Private credential storage is installation/tunnel scoped and opt-in; tests check
permissions and unsafe filesystem entries. Saved keys are plaintext local files
protected by filesystem permissions, not encrypted vault entries. No live keys
are included in fixtures or this repository.

Legacy releases through v0.4.0 did not persist the runtime key. A stopped legacy
runtime cannot supply it through the supported recovery mechanism; one final
user entry and explicit local storage consent may therefore be necessary.

## Reproduce

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release
for suite in test_release test_verify_public_release test_installer; do
  python3 -W error::ResourceWarning scripts/$suite.py
done
for suite in test_mcp test_generic_mcp test_onboarding test_install_flow \
             test_zero_arg_ux test_journey_fixture test_complete_journey; do
  python3 -W error::ResourceWarning scripts/$suite.py target/release/tv-control
done
python3 scripts/check_architecture.py
python3 scripts/audit_publication.py
sh -n install.sh
git diff --check
```

Public macOS/Linux package verification remains a separate required release gate
as described in [RELEASING.md](RELEASING.md). These local results do not claim
physical-device qualification or a measured chat-to-TV latency.
