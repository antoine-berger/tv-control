# tv-application

Use cases and ports. This crate depends only on `tv-domain`.

- [`src/lib.rs`](src/lib.rs): `Control`, device/catalogue/state/clock/cancellation
  ports and `Service`; reservations, dispatch and postcondition verification.
- [`src/onboarding.rs`](src/onboarding.rs): network-discovery port, setup planning and refresh rules.
- [`src/pipeline.rs`](src/pipeline.rs): staged connection orchestration.
- [`tests/pipeline_contract.rs`](tests/pipeline_contract.rs): injected-port lifecycle
  contracts; control contracts also live in the adapter integration tests.

Keep I/O behind ports. One caller-owned deadline covers the operation; a successful
send is not a verified result. Preserve `outcome_unknown` after possible mutation.

From the repository root: `cargo test --locked -p tv-application` and
`python3 scripts/check_architecture.py`.
