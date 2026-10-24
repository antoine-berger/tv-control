# Bounded I/O readiness measurement

A local Linux x86-64 debug build based on `986e8db` was measured on 2026-10-24.
The experiment runs 100 independent `/bin/sh -c 'printf ok'` subprocesses through
`BoundedProcess` with its normal 10 ms cancellation quantum. It uses no TV,
ADB executable, tunnel, credentials or external network.

| Implementation | p50 | p95 | max |
| --- | ---: | ---: | ---: |
| Fixed sleeps (baseline) | 10.278 ms | 10.450 ms | 10.581 ms |
| Readiness polling | 0.685 ms | 1.945 ms | 2.047 ms |

These are observations, not performance guarantees or physical-device timing.
Uncontrolled host scheduling affects the results; release builds and real devices
need separate measurements. The improvement removes a fixed sleep when a short
process or ADB smart-socket response is already ready. It does not cache remote
state, skip identity checks, enlarge deadlines or replay commands.

Reproduce from the matching checkout with:

```sh
cargo test --locked -p tv-adapters --test process_contract \
  short_process_latency_measurement -- --ignored --nocapture
```

Regression tests independently exercise ready output with a cancellation quantum
longer than the operation deadline, large stdin backpressure and EOF, fragmented
smart-socket responses, truncated responses, process cancellation, deadlines,
closed output with a still-running child and inherited descendant pipes. Poll
wakeups always return to the original deadline/cancellation checks. Closed pipes
are removed from the readiness set to avoid spinning on persistent hangup events.
