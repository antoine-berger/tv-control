# tv-control CLI

The executable composes adapters and exposes terminal commands and MCP stdio.

- [`src/main.rs`](src/main.rs): command dispatch, configuration loading and service construction.
- [`src/mcp.rs`](src/mcp.rs): MCP negotiation, tool calls, response bounds and cancellation.
- [`src/onboarding.rs`](src/onboarding.rs) and [`src/onboarding/`](src/onboarding):
  discovery/setup options, terminal wizard, dependency prompts and saved progress.
- [`src/journey.rs`](src/journey.rs) and [`src/journey/`](src/journey):
  local access, managed runtime, tunnel setup and conversation verification.
- [`src/terminal.rs`](src/terminal.rs): terminal input and hidden secret entry.
- [`src/journey_fixture.rs`](src/journey_fixture.rs): simulated journey entry point;
  fixture results must not claim real readiness.

Keep presentation here and device/process mechanics in adapters. Prompts must
preserve the difference between configuration, approval and verified access.
MCP stdout is protocol-only; do not add terminal messages to it.

From the repository root:

```sh
cargo test --locked -p tv-control
cargo build --locked --release
python3 -W error::ResourceWarning scripts/test_mcp.py target/release/tv-control
python3 -W error::ResourceWarning scripts/test_complete_journey.py target/release/tv-control
```

Run the [full checks](../../docs/DEVELOPMENT.md#checks) before integration. The
[CLI reference](../../docs/CLI.md) owns command documentation.
