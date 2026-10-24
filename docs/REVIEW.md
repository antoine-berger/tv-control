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
# Guided runtime qualification

The v0.3.0 composition has isolated terminal/process fixtures, including the
zero-argument piped installer, cooperative MCP verification and owned-runtime
stop. These results do not qualify an actual TV, account, hosted conversation or
router. New TLS pairing and automatic tunnel provisioning are outside this flow.

Normal cancellation and supervised termination clean the scoped ADB child.
Abrupt SIGKILL, process abort or host failure cannot run Rust cleanup; an orphaned
ADB process may then occupy the dedicated port. The adapter refuses that occupied
port rather than adopting or killing an unknown process. Recovery after such a
failure still needs explicit diagnosis. The free-port check and foreground start
also have an OS scheduling gap; no daemon-manager operation is used to repair it.

## Extended generic controls (v0.4.7 source candidate)

The extended batch combines remote navigation, Jellyfin catalogue/exact playback,
last-observation diagnostics and the source-size gate with the existing strict
session parser and focused-text implementation. These are integration contracts,
not live device qualification:

- Long-select follows the same UI-reference invalidation as navigation, including
  a rejected syntax probe. Old activation/text references cannot be revived by an
  identical-looking screen; remote capability reads never inject a test key.
- A completed catalogue read must still pass expiry/cancellation checks before
  an already-satisfied paused seek can return a verified no-op.
- Native media commands retain exact session identity and advertised action-mask
  checks. Jellyfin item/source controls remain separate, capability-gated adapter
  operations; no app names or model-specific fallback enter the generic core.
- Bounded uncertain diagnostics discard media item/source identities and position.
  Durable complete receipts can contain source identity: follow the coordinated
  [journal upgrade boundary](MCP.md#shared-journal-upgrade-boundary). Never delete a
  journal or replay an uncertain operation to recover from an upgrade problem.
- Every individual tool fits the minimum 4,096-byte output frame. The aggregate
  catalogue is paginated; the isolated installer proof journey checks all six
  tools, including `tv_media_read` and `tv_verify_connection`, across those pages.
- The maintained-source size policy covers the integration tests as well as
  production code. Remote adapter regressions stay in their cohesive test module,
  alongside the existing UI, media, power and volume modules.

Offline tests use injected executables, clocks and loopback services. They do not
qualify a real Jellyfin API, client, TV, macOS installation, physical playback,
remote-key acceptance or speaker silence. Publishing, cross-platform CI and
hardware acceptance are separate gates from local source integration.

## Standby and fixture ownership (v0.4.8 source candidate)

Standby uses only the dedicated Android sleep key after the documented platform
and syntax qualification, never a power toggle. Its read-only power evidence and
verified receipts describe Android wakefulness, not physical panel darkness,
external audio silence or a universally supported TV behavior. Unsupported
qualification remains explicit; ambiguous post-dispatch state remains uncertain
and must not be replayed. Standby also invalidates old activation/text references,
including when qualification rejects the platform. No live standby or sound
qualification is implied by fixture success.

Journey cleanup uses the private fixture's owned shutdown handshake instead of
signalling stale saved process IDs or groups. The integrated terminal and runtime
fixtures exercise this ownership boundary without touching a real TV or account.

## Read-only app details (v0.4.9 source candidate)

App details describe the freshly discovered current-user app, its active package
version and package-scoped native SEARCH resolver evidence. Hidden factory
versions, ambiguous records, failed reads and unknown output formats cannot
become affirmative capability evidence. A resolved handler is not proof that an
app accepts a query, displays results or can be controlled through that intent.

The read does not launch an activity, modify search dispatch/configuration, cache
capabilities, reserve a mutation journal or invalidate an existing UI reference.
The cross-feature regression preserves reference activation after reading details
and checks that handler evidence does not rewrite the selected app's search field.
The generic core has no per-app fallback or hardcoded application exceptions.
Existing minimum-frame paginated discovery and installer proof checks remain
required. These offline results do not qualify actual app behavior on hardware.
No native text helper prototype is included in this source candidate.

## Explicit media keys and input inventory (v0.4.10 source candidate)

System-global media keys are explicit requests, never fallbacks for session-scoped
commands. Dispatch does not identify the receiving app or prove playback. Input
inventory is a fresh read-only TIF observation; it neither selects a source nor
controls CEC, playback, audio or power. Saved sanitized dumps are parser fixtures,
not evidence that a live command succeeded on another device.

Failure classification is typed independently of diagnostic strings. Combined
regressions preserve unsupported remote-key qualification, uncertain dispatch,
and the difference between unsupported input formats and truncated reads. Codes,
effects, wire vocabulary and historical journal receipts retain their semantics.
Follow the coordinated shared-journal reader upgrade instructions for the new
intent; never delete uncertain-operation history or replay to repair an upgrade.

This candidate includes no native text helper/prototype. Offline tests and
minimum-frame paginated discovery/installer proof do not qualify hardware media
keys, input switching, CEC or physical playback. The multi-digit patch version is
covered by the normal installer/release/verifier contract suite.
