# Examples

These files use fictitious identifiers and reserved documentation addresses.
They are inputs for learning and offline checks, not a working TV installation.

- [`config.json`](config.json): ADB and Jellyfin aliases, app/media/workflow entries
  and explicit resource bounds. See [configuration](../docs/CONFIGURATION.md).
- [`discovery.json`](discovery.json): simulated LAN interfaces and discovery packets.
- [`mcp.json`](mcp.json): client configuration template; set your actual executable
  and private configuration paths before use. See [MCP](../docs/MCP.md).

From the repository root, after building:

```sh
./target/release/tv-control validate examples/config.json
./target/release/tv-control discover --fixture examples/discovery.json \
  --interface lan-fixture --json
```

These commands do not contact a TV. Do not commit a populated local configuration,
credentials, real device IDs or viewing history. Use the installer to create private
setup, or copy/edit the configuration outside the repository.
