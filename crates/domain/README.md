# tv-domain

Dependency-free vocabulary shared by application and adapters.

- [`src/lib.rs`](src/lib.rs): actions, requests, observations, evidence and receipts.
- [`src/onboarding.rs`](src/onboarding.rs): discovery/setup and access-state types.
- [`src/pipeline.rs`](src/pipeline.rs): connection pipeline stages and outcomes.

Keep these types independent of JSON, paths, processes, environment variables and
protocol clients. Add behavior only when it expresses a domain rule; host-specific
validation belongs outside this crate.

From the repository root: `cargo test --locked -p tv-domain`.
See [application](../application/README.md) for the use cases consuming these types.
