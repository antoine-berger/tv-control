# Acceptance and measurement

The end-to-end target is **user submission → requested physical TV effect and
displayed confirmation within ten seconds**, under declared healthy conditions.
It is not yet proven. Outages, model scheduling, authentication dialogs and media
buffering prevent an unconditional upper bound.

Label every result by scope: fake clock, loopback fixture, controller-to-device,
or full user-to-physical-effect-and-response. Do not combine these measurements.

## Required evidence

| Scenario | Evidence and acceptance criterion |
| --- | --- |
| State read | Correct device and fresh receipt; report observed latency without a playback claim |
| Pause/resume | Appropriate playback evidence, including position progress for Jellyfin resume, before the original expiry |
| Configured media resume | Exact user/device/item, expected seek range and increasing position; qualify physical output separately for the end-to-end target |
| App launch | Exact foreground app; no title or playback claim |
| Volume | Requested Android stream index and matching calibration; no inferred TV percentage |
| Concurrent commands | One reservation per physical-device group in a process; contention returns busy without queued late mutation |
| Duplicate request | At most one dispatch while the entry is retained; completed replay is marked cached |
| Expired request | Rejected before dispatch with zero device actions |
| Cancel during read | Owned process group cleaned up; cancelled without mutation |
| Cancel/timeout after possible dispatch | Uncertain receipt without false success or automatic replay |
| Frozen progress or buffering | No verified playback postcondition; count unknown or late playback as target failures |
| Malformed configuration or selector | Rejected before mutation; no request-supplied shell, URL or host override |

These criteria do not extend process-local coordination across restarts or other
controllers. See [request semantics](CLI.md#deadlines-and-retries).

## Hardware trial design

Declare initial readiness, app/server versions, network conditions, warm/cold
state, audio route and permissions. For an initial report, use at least 100 trials
per qualified scenario and report p50, p95, maximum and failure rate. A finite
observed maximum is not a universal bound.

Report cold app starts, wake/reboot, buffering and authentication as separate
cohorts. Do not exclude slow trials after observing their results. Count unknown
outcomes and late playback as failures of the ten-second target.

Record user submission, host tool dispatch, local receipt, device dispatch,
physical effect, verified observation and displayed confirmation. Only a trusted
host/client can supply the submission timestamp; the controller cannot infer it.
Compare clock domains explicitly and use a common monotonic observer where
possible. Client-reported progress does not establish physical video/audio output;
qualify that path separately without continuous captures or DRM workarounds.

## Hardware procedure

Confirm target identity and access, observe current app/playback/audio state, and
avoid disrupting an active user. Run only separately authorized bounded,
reversible actions and restore the initial state where safe and observable.
Stop if identity changes, the user takes over or state becomes ambiguous.

Performance testing does not include loud audio, purchases, deletions, account
selection, pairing, persistent authentication or router/network modifications.
Keep real configurations and evidence private and out of Git. Controller-only
[host benchmarks](../benchmarks/README.md) require no hardware access.
