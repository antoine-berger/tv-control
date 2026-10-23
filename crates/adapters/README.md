# tv-adapters

Concrete device and host integrations. The application owns policy; this crate
handles serialization, bounded I/O and private persistent state.

| Area | Files |
| --- | --- |
| Device control | `adb.rs`, `adb_server.rs`, `jellyfin.rs`, `http.rs`, `lib.rs` |
| Configuration and wire format | `config.rs`, `wire.rs` |
| Process/stdio and in-memory state | `process.rs`, `stdio.rs`, `runtime.rs` |
| LAN discovery | `discovery/` (DNS parsing, network source and fixtures), `setup.rs` |
| Saved choices | `setup_checkpoint.rs`, `pipeline_store.rs` |
| Managed connection | `onboarding_runtime.rs`, `onboarding_tunnel.rs`, `runtime_intent.rs`, `connection_proof.rs` |
| Approved helper acquisition | `dependency_acquisition/`, `dependency_acquisition.rs`, `tunnel_dependency.rs` |

Paths above are relative to `src/`. [`tests/`](tests) contains contract suites named
for their area. Unit tests beside implementations cover private helpers; optional
archive-inspection tests are documented in [development](../../docs/DEVELOPMENT.md#connection-dependency-checks).

From the repository root: `cargo test --locked -p tv-adapters`.

When changing a boundary, test timeout/cancellation, oversized input, uncertain
dispatch and cleanup as well as success. File-backed adapters also need ownership,
permission, link, interrupted-write and resume coverage. Never use real TV or
account access in an ordinary test. See [adapter contracts](../../docs/ADAPTERS.md).
