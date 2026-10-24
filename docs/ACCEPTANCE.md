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
| Concurrent commands | One reservation per physical-device group across processes sharing the same journal; contention returns busy without queued late mutation |
| Duplicate request | At most one dispatch while the entry is retained; completed replay is marked cached |
| Expired request | Rejected before dispatch with zero device actions |
| Cancel during read | Owned process group cleaned up; cancelled without mutation |
| Cancel/timeout after possible dispatch | Uncertain receipt without false success or automatic replay |
| Frozen progress or buffering | No verified playback postcondition; count unknown or late playback as target failures |
| Malformed configuration or selector | Rejected before mutation; no request-supplied shell, URL or host override |

Completed receipts and interrupted-operation uncertainty persist across restarts
in the configuration directory's private operation journal. Cross-process resource
exclusion applies only to controllers sharing that journal; it does not cover
physical remotes, independent controllers or copied configurations with different
state directories. See [request semantics](CLI.md#deadlines-and-retries).

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

For playback trials, use explicitly approved public media or dedicated test
fixtures. Do not use personal watch-progress content without the user's explicit
choice of that content and the test. Before starting, set a bounded playback
window and arrange independent, authorized cleanup that does not depend on another
model turn or user reply. Never leave private media playing while waiting for
either. Stop the trial immediately on user intervention; do not compete with a
physical remote or replay an uncertain command. If cleanup cannot be established
safely, do not start playback. Do not automatically reset watch history or progress
afterward: those are separate user-data changes, not harmless test cleanup.

Preserve the existing mute state and audio routes; do not use sound as an implicit
success signal. When the user requires a sleeping TV to remain asleep, restrict
qualification to permitted read-only checks and never wake it to complete QA.
A general test request does not override these constraints. Keep all private media
titles, filesystem paths, device/account identifiers and captures out of public
reports and repository fixtures.

Performance testing does not include loud audio, purchases, deletions, account
selection, pairing, persistent authentication or router/network modifications.
Keep real configurations and evidence private and out of Git. Controller-only
[host benchmarks](../benchmarks/README.md) require no hardware access.


## Exploratory URL search evidence

An authorized manual check on 2026-10-24 used controller v0.4.4 with Android 12
Google TV and the discovered YouTube TV app. A SEARCH intent was ignored, while
an explicitly package-targeted YouTube `/results?search_query=...` URL displayed
search results in a screenshot. UI Automator exposed blank View nodes on that
screen; a native editable-field flow was not established. Installed/system app
metadata listed differing versions, so the active YouTube version was unresolved.

This is a single exploratory observation, not version-qualified compatibility,
current-source hardware acceptance, a latency benchmark, or playback/audio proof.
The generic URL route remains caller-selected and dispatch-only. Qualifying a
release requires resolving the active app version and repeating the route with
that exact controller build under the hardware procedure above. Do not infer
support for blind text entry or automatic SEARCH fallback from this observation.
