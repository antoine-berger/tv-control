# Measurement scope

`scripts/benchmark.py` measures a native host binary without contacting a device:
cold CLI validation, warm MCP ping round trips, and idle process RSS/CPU. It records
compiler, OS/architecture, sample count, binary bytes and explicit timing scope.
Run it after a release build; stdout can be saved to a sanitized report. CPU is
average process CPU time over an idle window, including startup and shutdown,
not a claim about a router or the whole ADB/tunnel stack.

```sh
python3 scripts/benchmark.py target/release/tv-control > benchmarks/host.json
```

Do not publish real device traces here. Private runs belong in ignored `.local/`.
Controller-only synthetic results cannot establish chat→TV ten-second performance.

## Initial native host observation

The checked-in `host-macos-arm64.json` records one Rust 1.99.1 release build:
this snapshot predates the independent-review changes and identifies its source
commit explicitly. It does not measure the new stdio/probe overhead.
603,456 binary bytes, idle RSS maximum 2,129,920 bytes, server RSS peak 3,063,808
bytes over 20 loopback Jellyfin command fixtures, and idle-window CPU average
0.064% of one core. The 20 fixture round trips had median 140.96 ms, p95 170.56 ms
and maximum 179.99 ms. Cold CLI configuration validation had median 5.52 ms over
50 samples. Warm MCP pings had median 0.0195 ms over 1,000 samples.

These are descriptive observations from one host/run, not regression thresholds,
router results or physical media timings. Controller RSS excludes separate curl,
ADB daemon and tunnel processes. OS scheduling and unrelated load were not
controlled. Optimization profile comparisons remain a future experiment.
