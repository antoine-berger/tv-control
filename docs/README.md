# Documentation

## Install and use

1. [Installation](INSTALLATION.md): prerequisites, install/resume/uninstall,
   dependency acquisition and troubleshooting.
2. [Onboarding](ONBOARDING.md): TV authorization, managed runtime and ChatGPT
   conversation verification.
3. [Configuration](CONFIGURATION.md): device aliases, app/media settings and bounds.
4. [CLI](CLI.md) and [MCP](MCP.md): commands, tool schemas, deadlines and receipts.
5. [Adapters](ADAPTERS.md): supported behavior and evidence limits.

## Develop and qualify

- [Development](DEVELOPMENT.md): source builds, checks and contribution guidance.
- [Source map](../crates/README.md): where code and tests belong.
- [Architecture decisions](ADR.md): design rationale and trade-offs.
- [Acceptance](ACCEPTANCE.md): real-device and integration qualification.
- [Router](ROUTER.md): cross-build and deployment constraints.
- [Benchmarks](../benchmarks/README.md): measurement scope and reproduction.
- [Releasing](RELEASING.md): immutable assets, verification and channel promotion.
- [Review](REVIEW.md) and [plan](../PLAN.md): regression history and remaining work.

Reference pages describe the checked-out source. Published release assets may
come from an earlier commit. Keep claims tied to their tested environment; fixture
success does not establish TV, hosted-chat or router acceptance.
