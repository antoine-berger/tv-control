# Host benchmarks

These measurements describe the controller on one macOS host. They do not include
a physical TV, hosted chat, ADB daemon or tunnel, and do not qualify router runtime
or the ten-second end-to-end target.

## Procedure

After a release build, run from the repository root:

```sh
python3 scripts/benchmark.py target/release/tv-control > benchmarks/local-host.json
```

The output path above is ignored by Git. Review and sanitize any report before
publishing it. Real device traces belong in private storage, such as ignored
`.local/`, rather than this directory.

Pass `--compiler-version` with the compiler string used to build the measured
binary; otherwise that field is `not_recorded`. Record the source commit alongside
the output, since the script does not add it automatically.

The script records OS/architecture, binary bytes, cold CLI validation,
warm MCP ping latency and idle process RSS/CPU without contacting a device. CPU
is average process CPU time over an idle window, including startup and shutdown.
A loopback Jellyfin fixture additionally measures local protocol and postcondition
work; it does not play media on a physical client.

## Historical snapshot

[host-macos-arm64.json](host-macos-arm64.json) records a Rust 1.99.1 release build
of source commit
[`97c2596`](https://github.com/antoine-berger/tv-control/commit/97c25965954b25085f9d2ae62f8a8d151a1e3ba7).
**It predates later corrections, including stdio and ADB-probe changes.** It is not
a benchmark of the current revision.

| Measurement | Samples | p50 | p95 | Maximum |
| --- | --- | --- | --- | --- |
| Cold CLI configuration validation | 50 | 5.52 ms | 5.81 ms | 6.54 ms |
| Warm stdio ping | 1,000 | 0.0195 ms | 0.0215 ms | 0.1202 ms |
| Local stdio → loopback Jellyfin fixture | 20 | 140.96 ms | 170.56 ms | 179.99 ms |

The binary was 603,456 bytes. Idle RSS maximum was 2,129,920 bytes; controller RSS
peak during loopback commands was 3,063,808 bytes. Idle-window CPU averaged about
0.064% of one core.

These are observations from one host/run, not regression thresholds. Controller
RSS excludes separate curl/ADB clients, the ADB daemon and any tunnel. OS
scheduling and unrelated load were not controlled. Optimization-profile comparisons
and measurements on actual router firmware remain future work. Use the
[acceptance protocol](../docs/ACCEPTANCE.md) for user-to-physical-effect claims.
