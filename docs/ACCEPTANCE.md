# Acceptance and measurement protocol

Every result must identify scope: fake clock, loopback fixture, controller→device,
or user submission→physical TV effect→displayed response. Do not combine them.

| Scenario | Required evidence | Acceptance target |
| --- | --- | --- |
| State read | Correct device, fresh receipt | Report observed latency; no playback claim |
| Pause/resume | Appropriate playback state; server resume progress | Verified before original expiry |
| Configured media resume | Exact user/device/item, expected start position, increasing position, physical output qualification | User-to-effect and displayed confirmation ≤10 s in declared healthy conditions |
| App launch | Exact foreground app | No claim of title or media playback |
| Volume | Configured index and matching observed calibration | No conversion to TV percent without device evidence |
| Concurrent commands | One reservation per device; different ID rejected busy | No queued late mutation |
| Duplicate ID | Dispatch count remains one; marked receipt replay | No repeated mutation |
| Expired queued request | Rejection before dispatch | Zero device actions |
| Cancel during read | Owned process group cleaned up | Cancelled, zero mutation |
| Cancel/timeout after dispatch | Distinct uncertain receipt | No false success or automatic repeat |
| Buffering/frozen progress | No progress postcondition | Target failure/unknown at deadline |
| Malformed config/selector | Input rejected before mutation | No shell injection or host override |

For each hardware scenario, declare initial readiness, app/server versions, network
conditions, warm/cold state, audio route and permissions. Use at least 100 trials
per qualified scenario for an initial p50/p95/max and failure-rate report. A finite
observed maximum is not a universal upper bound. Count unknown and late playback
as failures of the target. Cold app start, wake/reboot, buffering and auth need
separate cohorts rather than exclusion after observing slow results.

Record user send, host tool dispatch, local receipt, device dispatch, physical
effect, verified observation and displayed confirmation. Only a trusted host/client
can supply the first timestamp; the controller cannot infer it. Compare clock
domains carefully and use a common monotonic observer where possible. Native
client-reported progress does not prove the physical screen/audio path; qualify
that path separately without continuous screenshots or DRM workarounds.

Hardware preflight: confirm target and access, observe current playback/app/audio,
avoid disrupting an active user, run bounded reversible actions, and restore the
initial state where safe and observable. If identity changes, the user takes over,
or state is ambiguous, stop. No loud audio, purchase, deletion, account selection,
pairing, persistent authentication or router/network modification belongs in a
performance test. Keep real configs and evidence private and out of Git.
