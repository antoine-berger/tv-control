# Regression notes

The initial MVP was published as
[`97c2596`](https://github.com/antoine-berger/tv-control/commit/97c25965954b25085f9d2ae62f8a8d151a1e3ba7).
The following safeguards and regressions were added in
[`adc796e`](https://github.com/antoine-berger/tv-control/commit/adc796eab92448314c38ba70124723d4ffb4e380).
This record describes code and fixture coverage; consult
[current CI](https://github.com/antoine-berger/tv-control/actions) for each revision.

| Issue | Safeguard and regression scope |
| --- | --- |
| Missing/null Jellyfin positions treated like native playback proof | Explicit playback evidence; adapter/use-case regressions and stdio/HTTP fixtures for missing, null, frozen and progressing positions |
| Verification polling silently capped at 50 ms | Full configured wait in cancellable slices; fake-clock 500 ms, cancellation and deadline cases |
| Blocked stdout prevented cancellation and teardown | Bounded nonblocking output and input; unread-output flood with bounded exit |
| Aliases bypassed exclusion for one physical screen | Shared coordination groups and duplicate-target validation; concurrent two-alias reservation |
| ADB client could kill an incompatible daemon | Read-only version preflight and fake incompatible-server fixture; concurrent replacement race remains |
| Resume-media accepted far-ahead playback | Configured seek tolerance plus elapsed time; near-start/progress and far-ahead cases, with fixture seek applied |
| Session tag collided across packages | Require global tag uniqueness before monitor dispatch; duplicate-tag case asserts zero mutation |

The ADB guard is not an unconditional no-restart guarantee. Use an exclusively
owned daemon and retain the [check/use race warning](ADAPTERS.md#adb-daemon-ownership).
Automated fixtures do not qualify TV output, hosted access or router execution.
The checked-in [macOS benchmark](../benchmarks/README.md) predates these changes.

## MCP discovery output floor

[`54362a5`](https://github.com/antoine-berger/tv-control/commit/54362a51de87cc00665a2e54b489b1ebf7fad077)
raises the minimum `max_output_bytes` to 4,096. The previously accepted 1,024-byte
limit could close the transport while serializing `tools/list`. Regressions reject
smaller configurations and exercise initialization, tool discovery and ping at
the accepted minimum. Configured capability listings can still require more space.

## CI fixture details

An initial macOS failure came from a content-limit test sharing a one-second
Python startup budget. Independent five-second contexts isolate content assertions
from startup variation; dedicated timeout tests and production deadlines remain
separate. The CI matrix keeps other jobs running after a failure for diagnostics.

The inherited-stdout regression checks both blocking and nonblocking input flags
and restores the complete captured flags. On macOS, the first pipe write sets
XNU's kernel-only `FWASWRITTEN` (`0x10000`), which is outside the flags settable by
`F_SETFL`. The test primes the pipe before taking its baseline and compares all
bits without masking, distinguishing write history from a leaked `O_NONBLOCK`.
See [XNU fcntl definitions](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/fcntl.h).
