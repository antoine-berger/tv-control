# Source map

Four crates separate policy from host/device I/O. Read [development](../docs/DEVELOPMENT.md)
for build/test commands and [architecture decisions](../docs/ADR.md) for rationale.

| Crate | Owns | Start here |
| --- | --- | --- |
| [domain](domain/README.md) | Requests, actions, evidence, onboarding and pipeline states | `src/lib.rs` |
| [application](application/README.md) | Use cases, ports, deadlines and verification | `src/lib.rs` |
| [adapters](adapters/README.md) | ADB/Jellyfin, process I/O, discovery, persistence and helpers | `src/lib.rs`, `src/config.rs` |
| [cli](cli/README.md) | Command parsing, terminal journey and MCP composition | `src/main.rs` |

Dependencies point inward: domain has none; application depends only on domain;
adapters implement application ports; CLI wires them together. Run
`python3 scripts/check_architecture.py` from the repository root after moving code.

For a behavior change, start at its application port and contract test. Put
platform-specific mechanics in adapters and user interaction in CLI. Keep
request expiry, uncertainty after dispatch and private runtime ownership intact.
