# Generic ADB control validation

Validated on Linux x86-64 on 2026-10-23. No physical TV, Mac installation,
public release, or production bridge was modified during this validation.

## Results

- Rust workspace: **262 passed, 0 failed, 2 ignored**.
- Python suites: **177 tests, 176 passed, 1 skipped** (requires a separately supplied verified v0.2.2 binary).
- Formatting, workspace Clippy with warnings denied, architecture boundary check,
  shell syntax check and diff whitespace check passed.
- Optimized Linux executable: **1,566,952 bytes** (toolchain/build dependent).
- Independent review found no outstanding safety blocker.

| Python suite | Tests |
| --- | ---: |
| `test_generic_mcp.py` | 4 |
| `test_mcp.py` | 15 |
| `test_complete_journey.py` | 42 |
| `test_installer.py` | 28 |
| `test_release.py` | 13 |
| `test_verify_public_release.py` | 18 |
| `test_onboarding.py` | 29 |
| `test_install_flow.py` | 9 (1 skipped) |
| `test_zero_arg_ux.py` | 12 |
| `test_journey_fixture.py` | 7 |

The two ignored Rust cases require explicitly supplied official Google ADB and
tunnel ZIP archives; they verify offline extraction without executing payloads.
Those archives were not supplied. The skipped Python case specifically checks
installer compatibility with a separate verified v0.2.2 executable, which was
not supplied. Current-version installer and upgrade fixtures passed.

## Reproduce

From the repository root with Rust and Python 3 installed:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release
for suite in test_generic_mcp test_mcp test_complete_journey \
             test_onboarding test_install_flow test_zero_arg_ux test_journey_fixture; do
  python3 -W error::ResourceWarning scripts/$suite.py target/release/tv-control
done
python3 -W error::ResourceWarning scripts/test_installer.py
python3 -W error::ResourceWarning scripts/test_release.py
python3 -W error::ResourceWarning scripts/test_verify_public_release.py
python3 scripts/check_architecture.py
python3 scripts/audit_publication.py
sh -n install.sh
git diff --check
```

The tests use deterministic fake ADB processes, local protocol fixtures, a real
compiled Android resource table, and real CLI/MCP binaries. They cover durable
replay/recovery, cancellation and cross-process exclusion, consent upgrades,
exact label/component resolution, cache invalidation, stale UI rejection,
shell quoting, targeted observations, and legacy aliases. Image tests generate
1080p/4K PNGs above the normal text-frame budget and verify the separate native
image lane; a valid oversized 4K image fails cleanly without breaking transport.

## Qualification still required

Actual Google TV and Fire OS hardware must validate firmware-specific ADB
package/media/UI output, search handlers, screenshot restrictions, and cold/warm
latency. These fixture results are not a user-to-TV latency guarantee.

Labels require the device's `unzip` command and supported base-APK resources;
split/overlay or unsupported resource qualifiers may leave labels missing.
Missing labels remain explicit and read-only discovery is retryable; exact
package/component selection is available without invented names. Search is only
exposed for discovered supported handlers and never promises arbitrary content
playback. Screenshot reads require explicit opt-in, accept PNGs up to 1 MiB,
and may fail on protected screens or larger captures. Normal reads never return
images. Exact activity launch is acknowledged with package-level foreground
evidence, not a claim that the activity itself was observed.
