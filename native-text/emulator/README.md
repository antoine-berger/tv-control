# Source-only owned-emulator runtime proof candidate

**The first reviewed workflow was published and stopped safely at its KVM access
preflight; no Android runtime ran. This software-backend correction awaits renewed
review before publication. All generated APKs are emulator-only test artifacts,
not user-installable releases. The separate Darwin candidate below requires an
explicitly selected private local lab; no user's TV is addressed or controlled.**

## Modern architecture

The former legacy helper mutation body is now shared `TextEngine.java`, using only
public Android APIs. `SetText.java` is a thin deprecated research wrapper and is
not included in either modern APK. There is one mutation engine, not two.

`modern/AndroidManifest.xml` defines a no-activity, no-service, no-provider,
no-permissions APK with Instrumentation targeting **its own helper package**. It
uses `Instrumentation.getUiAutomation(FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)`
and the active root's public input-focus API. It does not run the old UiAutomator
runner, call `setRunAsMonkey`, or write IME/locale/accessibility secure settings.
The view-ID flag changes only its temporary automation connection's service info.
No reflection or AndroidX dependency is involved.

This is not permission to install on the user's TV. That would require explicit
new helper-install approval, safe trusted transport/reference issuance, review,
and OEM validation. None of those is claimed here. The private cache request path
is intentionally not a completed host transport: ordinary shell cannot write the
app's private cache, and no debug/run-as bypass, exported receiver/provider, or
new persistent access is added. The runtime proof creates the request from inside
the owned test instrumentation process, then invokes the exact shared engine.

Public APIs:
- https://developer.android.com/reference/android/app/Instrumentation#getUiAutomation(int)
- https://developer.android.com/reference/android/app/UiAutomation#FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES

## Proposed test

Build three separately reviewed APKs with official SDK aapt2/D8/apksigner:

1. `fixture.apk`: no permissions; visible test-only native EditTexts. Its own
   dispatchKeyEvent deliberately maps A to Q, so `input text Tanya` must expose
   `Tqnyq`. This is a controlled reproduction, not an OEM keyboard claim.
2. `helper.apk`: shared engine plus modern Instrumentation, no fixture or proof.
3. `proof.apk`: same helper package and engine plus test-only Instrumentation.
   The proof targets itself and observes the separate fixture across the normal
   UiAutomation boundary; it never instruments a third-party app.

Install standalone helper and verify its missing-token refusal through real
`am instrument`. Replace it with the same-signed proof APK, and verify:
- deliberate keyevent mismatch;
- exact accessible Unicode/percent/quote append via ACTION_SET_TEXT;
- hint, password, visible-password, middle-selection, changed focus/window and
  cancellation refusal, no unintended editor change;
- consumed request refuses replay;
- default IME, selected subtype, enabled-accessibility and accessibility-enabled
  secure settings equal their initial snapshots after every run.

Password/hint/readback semantics still depend on accessibility metadata; this does
not assert access to app internals. Final validation/action IPC remains nonatomic.
Checking secure settings does not alone prove absence of transient suppression;
the modern connection explicitly requests the public do-not-suppress flag.

## Workflow boundaries and provenance

`.github/workflows/native-text-runtime.yml` runs only on a deliberate push to exact
`qa/native-text-runtime`, not main/tags/PRs. Token permissions are `contents: read`;
checkout is pinned and does not retain credentials. No artifacts, caches, APKs,
keys, UI dumps, screenshots or emulator logs are uploaded. The job only prints
fixed outcomes, public test failures and tool/package hashes/versions.

The official SDK URLs, archive byte sizes and metadata SHA1 pins are literal in
`provision.py`. Platform28 r6 and Build Tools35 additionally use the established
SHA256 pins. Runtime packages: platform-tools37.0.1, emulator37.2.12 build16428233,
AOSP/default API31 x86_64 system image r5. Their pins came from Google's official
repository XML; its runtime archive hashes currently use SHA1. This is disclosed,
not silently presented as stronger publisher-supplied SHA256 provenance.

Only pre-existing SDK license acceptance is checked. No `sdkmanager --licenses`,
`yes`, interactive acceptance, paid service or external account is used. Missing
licenses, JDK, KVM or package/hash mismatches fail closed. Hosted preinstalled JDK21
is recorded at build time, rather than pretending it is the locally pinned patch.
This runtime proof build is not a reproducible release package. The local pinned
legacy build remains separate; modern DEX repeatability is checked locally.
Fixture signing keys are newly generated disposable local test keys, deleted after
build, never uploaded or used for any account access. Signed APK hashes naturally
differ across builds because each build has a different disposable signing key.

KVM is used only when already accessible to the hosted runner. Otherwise an
explicit `--allow-software-emulation` selects the separate unprivileged
`-accel off` backend. Without that opt-in the runner fails. No KVM permissions,
ACLs/groups or sudo changes are performed, and inaccessible KVM is not opened.
The emulator has no window/audio/metrics and uses a fresh wiped AVD. Boot is
bounded to 240 seconds with existing KVM or 900 seconds in software; instrumentation
commands have 60-second host deadlines and the job has a 40-minute total deadline.
The shared helper's 5-second liveness gate and all mutation/assertion checks remain
unchanged. Emulator output is continuously drained while retaining at most 64 KiB
locally; none is uploaded. Commands are captured to bounded
runner-local files, terminated as owned process groups on timeout; cancellation
and final cleanup terminate only the owned emulator and uninstall only our fixed
package names. No retries of text mutation occur. This is not the shipping host
transport.

## Evidence / current blockers

Local environment has no adb, emulator or `/dev/kvm`, and insufficient `/tmp`
capacity for the pinned runtime image plus an AVD. Consequently Android runtime
behavior is **unexecuted**, not passed. All three APKs can be genuinely built and
signed locally against official SDK APIs; parent receives actual DEX hashes and
host-test results with the review candidate. A CI outcome must be checked before
claiming the instrumented helper works even on the owned AOSP API31 image.

### Local build evidence (not runtime evidence)

Built twice with installed javac 21.0.12.1 and pinned Platform28/BuildTools35:

| DEX | SHA256 (identical across both builds) |
|---|---|
| Fixture | `dbe23f9f53df3e90c8fa506e88fa093ea5fe6ad40e9435c87b8ca39748de55de` |
| Modern helper | `8a39a66562d2973a4462202a11add57ea14caaac75c06ce5defb30f08a34466c` |
| Test proof | `5f6ccf2478f5d42acd92f46477ba642865ead74fb8deb602605b99df082c3e1d` |

Local review APKs: `/tmp/tv-native-sdk/modern-build4/{fixture,helper,proof}.apk`.
Helper APK SHA256: `84e5065530271e005e419f1d85a344c7472ebb32f05816e195f33c976f76fe2d`.
Proof APK SHA256: `af1f18d0174da31e34fd46707f55e83f3a2cbd60a2b70309fb61305dbb667f9b`.
These binaries are not committed/uploaded/deployed. Binary manifest inspection
confirms the helper has only its self-targeting Instrumentation, no permissions
or activity/service/receiver/provider. Test signing key was removed after build.
Ten Rust tests, 21 source-extracted guard cases and 150 malformed-protocol Java
cases pass. They remain host/algorithm tests, not Android behavior.

### First CI result and proposed correction

The first exact reviewed tree ran at
https://github.com/antoine-berger/tv-control/actions/runs/40072790718 . JDK/license
checks passed; existing KVM access was unavailable, so the job failed before SDK
provisioning, APK build or Android startup. Cleanup passed. No permission gate
was changed at runtime.

The new source-only correction selects bounded software emulation when existing
KVM access is unavailable. Five host tests cover backend selection, required
opt-in, bounded log draining, and preserved security/audio boundaries. No helper
Java code, APK manifest, input assertion or SDK/license pin changes are involved.
A software boot timeout or runtime assertion failure will still fail the job.

### Owned process-group regression correction

Emulator liveness now uses Linux `waitid(..., WNOWAIT)` instead of `Popen.poll()`.
The parent retains the running or zombie leader until all group signals finish;
only then does `wait()` reap it. A reaped leader or lost child ownership refuses
all group signals rather than using a potentially recycled PGID. Seven explicit regressions (including real owned Python subprocesses) cover early launcher exit, ordinary exit, timeout, already-reaped
leader, lost-child ownership, non-reaping exit observation and bounded live-child
cleanup. No helper or text assertions were changed.

### Diagnostic-only candidate after the first software runtime

Run https://github.com/antoine-berger/tv-control/actions/runs/40074100060 verified
SDK/image pins, host tests and APK builds, booted a software API31 AVD, then reached
and failed the first `key-remap` fixture assertion. Standalone modern instrumentation
returned its expected missing-token/refused response before that mode. That alone
does not prove a connected UiAutomation session or successful ACTION_SET_TEXT.
Normal insertion and later refusal cases were not reached. Cleanup passed; ordinary
CI https://github.com/antoine-berger/tv-control/actions/runs/40074100040 passed.

The next patch is diagnostic only: the test instrumentation emits fixed enums for
its last stage (connection/idle/root/editor/remap/action/readback/cleanup), error
category, and a synthetic-only comparison category. It never emits field contents,
hierarchies, exception text or stacks. The host accepts only the exact allowlisted
schema, refuses duplicates/missing/invalid fields and prints only validated enums.
The synthetic comparison classifies the public fixture strings as expected-remap,
unchanged-literal, empty or other; it does not reveal arbitrary text. Six host tests
cover enum synchronization, success consistency, malformed outputs and non-echo.

The existing fixture uses `Activity.dispatchKeyEvent`, not an OnKeyListener. An IME
could consume a key before that post-IME path sees it. A custom owned EditText's
public `onKeyPreIme` hook is a possible deterministic-fixture correction if the
new stage evidence supports that hypothesis; ACTION_SET_TEXT does not generate
key events. No such fixture change is included now, and the expected `Tqnyq`
assertion is unchanged. No IME/accessibility settings are changed to force a pass.

Diagnostic candidate builds locally. Shared helper DEX remains unchanged at
`8a39a66562d2973a4462202a11add57ea14caaac75c06ce5defb30f08a34466c`;
proof-only DEX is
`3d0181b2bc79ae775828bd9dadd4ff11346ac4b6b8df3a7d6b5dd2e0e94e1556`.
No diagnostic candidate has been published or run yet; renewed review is required.

Editor-lookup diagnostics additionally expose fixed categories only: root package
is compared against the owned fixture/helper, AOSP Latin IME, and Android/systemui;
all other packages become OTHER (never printed). Match count is ZERO/ONE/MULTIPLE.
The proof reads its own REPORT_VIEW_IDS flag and public PowerManager.isInteractive /
KeyguardManager.isKeyguardLocked without writing settings or waking/unlocking.
Unavailable observations remain UNKNOWN. These observations do not change selectors,
assertions, or helper behavior. Activity-manager launch output is reduced to a fixed
category; FIXTURE_OK means a reported successful launch, not proof it stayed visible
or did not subsequently crash. No launch category bypasses the existing assertions.

### Bounded system-overlay / owned-error diagnosis

The test runner now recognizes AOSP dialog resource IDs without reading labels or
clicking: `aerr_wait` (ANR), `aerr_restart`/`aerr_app_info` (crash), ambiguous
`aerr_close` (error dialog), and `alertTitle`/`button1` (generic prompt). Resource
shape alone does **not** identify the affected app. Known source references:
[AppErrorDialog](https://android.googlesource.com/platform/frameworks/base/+/refs/tags/android-12.1.0_r27/services/core/java/com/android/server/am/AppErrorDialog.java),
[AppNotRespondingDialog](https://android.googlesource.com/platform/frameworks/base/+/refs/tags/android-12.1.0_r27/services/core/java/com/android/server/am/AppNotRespondingDialog.java).
These are best-effort AOSP signatures, not a universal dialog detector.

On failure only, after the exclusive fresh-owned-AVD gate, read at most 160 crash
and 160 system log records (10 seconds each, existing 128 KiB output bound).
AndroidRuntime/ActivityManager tag filters exclude other tags. Raw records are
transient local parser input, never printed, uploaded, or saved as an artifact.
The parser emits only exact owned package enums, allowlisted exception classes
(other classes become OTHER_EXCEPTION), and at most eight known project source
filename:line entries per crash. PID-bound Process records establish attribution;
ANR attribution requires an exact owned-package ANR record. The tail may be
truncated or unavailable; absence is not evidence of no crash/ANR. Events may
precede the current overlay, so report overlay shape and owned events separately.
No messages, system source frames, thread names, UI text, or arbitrary classes are
returned. Fixture/proof DEX retain source/line debug metadata only (no locals);
standalone helper compilation is unchanged. No setting changes, log clearing,
dialog dismissal, selector fallback, action replay, or altered assertions.

The existing pinned `system-images/android-31/default/x86_64` is already clean
AOSP, with PlayStore disabled and no Google APIs/GMS image. A GMS-to-AOSP switch
is therefore not an available explanation/fix. Software CPU/rendering pressure
remains only a hypothesis until diagnostics establish it; resolution/RAM/backend
changes would need a separately reviewed environment patch.

### Read-only fixture readiness before input

After each successful launch, a separate test-only `ready` instrumentation mode
polls accessibility for at most 20 seconds of device uptime, at 200 ms intervals,
with at most 100 probes and the existing 60-second host instrumentation deadline.
Success requires the active root package to be the fixture and exactly one editor
with the exact fixture package/resource ID/native EditText class, focused, editable,
enabled, visible, and nonempty bounds. Missing/system roots are observations to
wait through, never dismissed. Nodes/roots are recycled each probe. A probe that
finishes at/after the deadline cannot produce success. IPC may itself block; the
host deadline remains the outer bound.

This mode never reads/writes text, invokes accessibility actions, changes selection,
or consumes any request token. Hint/password/selection negative fixtures remain
eligible for readiness and still face all original engine checks afterward.
Readiness failure prevents the input command. Success permits only the original
single input invocation and then all original post-input assertions unchanged.
IME/accessibility secure settings are compared again after readiness. No promise
is made that readiness prevents the fixture losing focus afterward; that must still
fail the original proof rather than trigger mutation replay.

### Environment-only low-rendering-workload experiment

After the read-only readiness gate observed an ANR-shaped system overlay, the
owned AVD display is reduced from 1080x1920 at 420 dpi to 480x800 at 160 dpi.
This renders 384,000 rather than 2,073,600 pixels (about 81.5% fewer). It is only a
resource-pressure hypothesis, not a fix or typing-success claim. The existing
`hw.lcd.width`, `hw.lcd.height`, and `hw.lcd.density` AVD properties are supported
by the official [emulator hardware schema](https://android.googlesource.com/platform/prebuilts/android-emulator/+/8f496dcdc9aa1602c7905900d4292b4044bfc449/linux-x86_64/lib/hardware-properties.ini).
No new executable or flags are needed; no local emulator is available to claim a
local runtime result. Image, software/KVM selection, GPU mode, RAM/cores, budgets,
readiness predicate, APKs, all assertions, security settings and cleanup remain
unchanged. The dimensions apply only to the newly created disposable AVD config,
not through Android display-setting commands and never to a user's device.

### Read-only ANR window attribution

On failure only, one additional `dumpsys window windows` read is bounded to
10 seconds and the existing 128 KiB output cap. Raw data are transient parser
input, never printed/uploaded. Only full ANR window-header records are accepted.
AOSP [AppNotRespondingDialog](https://android.googlesource.com/platform/frameworks/base/+/refs/tags/android-12.1.0_r27/services/core/java/com/android/server/am/AppNotRespondingDialog.java)
sets the window title to a fixed ANR prefix plus the affected process name;
[WindowManagerService](https://android.googlesource.com/platform/frameworks/base/+/9697fd4/services/core/java/com/android/server/wm/WindowManagerService.java)
provides the read-only `windows` dump route. Output is reduced to exact owned
fixture/helper process enums, a fixed allowlist of known AOSP system processes,
OTHER, or UNKNOWN. OWNED_PROOF denotes the shared helper/proof package process,
not proof of which instrumentation entrypoint caused an ANR. Multiple categories
remain multiple; observed windows are not asserted to be the current foreground
or necessarily the same overlay from the earlier accessibility sample. Unsupported
format, absent records, timeout/permission failure, and output-budget overflow
remain unknown; there is no elevated fallback. Historical `activity lastanr`
state is deliberately not used to infer ownership from incidental activity names.
No diagnostics change readiness, mutation policy, settings, or cleanup. Existing
engine/readiness/host-instrumentation deadlines are unchanged; only failure-time
read-only diagnosis receives this additional independent 10-second bound.

## Darwin ARM64 candidate: review before runtime

The first local runtime attempt passed the HVF preflight but the foreground ADB
server aborted before readiness; no AVD was launched. Its cleanup receipt and
independent process/port checks found no SDK leftovers. Exact SDK ARM64 static
inspection and AOSP `socket_spec.cpp` established that the numeric listener host
`127.0.0.1` takes the unsupported-hostname branch before binding. The server now
uses the SDK's public `tcp:localhost:port` syntax, which calls the loopback helper
directly without DNS; clients and the emulator retain their numeric loopback
connection settings. This is not a successful Android runtime qualification.
Startup diagnostics now preserve only fixed error categories, bounded allowlisted
source locations and non-reaping exit metadata in the receipt before closing the
temporary SDK log. Only on ADB startup failure, an explicitly authorized private
tail of at most 8192 exact bytes is retained in `adb-startup-private-tail.log` using
exclusive, no-symlink creation with mode0600 in the owned mode0700 work directory.
It may contain confidential SDK diagnostics: do not print or publish its content.
Only its fixed basename, size, hash and truncation metadata enter the receipt.
Failed preservation makes cleanup unverified. No retries or permission fallback
are added. The fixed startup command still undergoes the same ownership guards.

Primary sources: [ADB socket specification](https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/socket_spec.cpp)
and [POSIX loopback implementation](https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/sysdeps/posix/network.cpp).
Current AOSP source is supporting evidence, not an assertion that its revision
matches the SDK release; the exact SDK binary's disassembly supplied that check.

The second attempt kept the foreground ADB process running but stopped at the
listener ownership guard before any AVD launch. The local `lsof -F?` contract
documents an always-emitted `f` descriptor field, even with `-Fpn`; the original
parser incorrectly rejected that field. The corrected parser requires one exact
PID header followed by complete, unique numeric descriptor/loopback endpoint
pairs. Wildcards, foreign PIDs, unknown fields, incomplete records and unexpected
ports remain rejected. The lsof command and process ownership checks are unchanged.
Both attempts completed cleanup verification; neither qualifies an Android runtime.

The third attempt passed the owned ARM64/HVF/ADB gates and the key-remap proof,
then failed the normal fixture's expected engine-status assertion. No Android
crash/ANR was reported and owned cleanup passed, but text insertion remains
unqualified. The proof now returns separate allowlisted first-call and replay
statuses: NOT_REPORTED, VERIFIED, REFUSED, UNCERTAIN, INVALID or MULTIPLE. It does
not expose arbitrary engine strings, UI text, tokens or exception messages.
The shared TextEngine has no finer error-code API; this diagnostic does not invent
an internal cause or claim an observed effect from a failed assertion. The engine,
assertions, four-second request deadline and UI calls are unchanged. Rebuilding
the test-only proof APK is required before the strict host parser can consume the
new fields. No helper distribution or TV qualification follows from host tests.

Source analysis identified a fixture/contract mismatch: AOSP API31 TextView returns
its hint for empty text, including null when no hint exists; the proof had converted
null to an empty string while TextEngine correctly rejects unobservable null text.
The positive fixture now tests append to the known public `seed` text with selection
at its end. It does not qualify inserting into an empty/null accessible field.
The new `empty-null` negative case explicitly requires null before invocation and
after refusal/replay; existing request-consumption and replay-refusal assertions
still apply. No engine, deadline, permission or identity check is relaxed.

| Fixture modes | Initial editor / selection | Expected engine result |
| --- | --- | --- |
| normal | `seed`, end4 | verified exact append, replay refused |
| empty-null | empty without hint, accessible null | refused, null unchanged, replay refused |
| hint | empty with public hint, accessible hint text | refused, unchanged |
| password / visible-password | public seed, end4, respective sensitive type | refused with nonnull text |
| selection | `seed`, position1 | refused, unchanged |
| focus | `seed`, end4; second editor `other`, end5 | refused after explicit focus switch |
| stale-window / cancel | `seed`, end4 | refused, unchanged |
| key-remap | empty without hint before synthetic key input | exact prior remap assertion |

The source reference is [Android12 TextView](https://android.googlesource.com/platform/frameworks/base/+/refs/tags/android-12.0.0_r1/core/java/android/widget/TextView.java),
`getTextForAccessibility()`. The fixture/proof/engine host checks use fake nodes and
exercise the actual methods as contract tests; they are not AOSP runtime evidence.

This candidate adds `--mode ci-linux` and `--mode darwin-local`; mode selection is
mandatory. Darwin requires a native ARM64 host, the official emulator's successful
HVF acceleration check, no software/Rosetta fallback, a private SDK/lab, and
pre-existing licensed SDK and reviewed APK build receipts. It does not download
tools, accept terms, change security settings, create startup services, or install
anything on a TV. The fixture assertions and shared text engine are unchanged.

The provisioning receipt records official Google archive URLs, published checksum
algorithm/value and size, locally computed SHA256, destination, and extracted-file
digests. SDK metadata currently publishes SHA1 for runtime archives; the local
SHA256 is not presented as an independently published pin. Runtime files and exact
AOSP API31/default/arm64 ABI are rechecked, including boot images and libraries.
Repository revision and archive `source.properties` revision remain separate
evidence: an official archive can contain different recorded revisions. No file
is rewritten to make them agree. The existing stable SDK license file must be
present, nonempty, owned and matched by the receipt; preview packages and analytics
opt-in are refused. The receipt is trusted local configuration, not a security
boundary against another process deliberately replacing both it and the SDK.

After independent review and explicit operator selection, a local invocation is:

```sh
python3.14 native-text/emulator/build_runtime.py \
  --sdk "$LAB/sdk" --java "$JAVA_HOME/bin/java" --out "$LAB/build-1"
python3.14 native-text/emulator/run_proof.py --mode darwin-local \
  --sdk "$LAB/sdk" --build "$LAB/build-1" \
  --provenance "$LAB/provision_receipt.json" --work "$LAB/run-1"
```

`LAB` must be an owned private directory; `run-1` must not exist. The build writes
an APK digest receipt. Host Python needs `waitid`/`WNOWAIT`; a Python without them
fails before starting SDK executables. SDK and build paths cannot escape the lab,
SDK-root symlinks and control characters in paths are refused, and runtime
provenance hashing is bounded to 90 seconds. The host needs at least 8 GiB RAM
and 8 GiB free storage. The AVD retains the 1536 MiB/two-core, low-resolution
configuration. Audio input/output, host cameras, snapshots, metrics and emulator
window are disabled; these are owned-AVD settings, not changes to a user's TV.

ADB runs as a directly owned foreground child with a fresh loopback smart-socket
port, a console/transport pair selected from available documented emulator ports,
and a private home/key/tmp/AVD environment. USB, mDNS auto-connect and emulator
auto-scanning are disabled. Every client invocation names the private host/port
and exact emulator serial. Read-only listener-PID, server-version and target-list
checks precede dispatch; another target or unknown ownership fails closed. The
emulator receives the same `ANDROID_ADB_SERVER_PORT` and socket environment.
Ports are reserved until launch; the unavoidable bind handoff is followed by
ownership checks before fixture installs or input.

The SDK ADB client can still auto-start/restart after a server check/use race; no
documented no-autostart client option is assumed. A missing owned foreground
server stops dispatch. Cleanup checks for leftover private ports and SDK processes
and fails explicitly instead of killing a PID found by scanning. Other independently
configured ADB servers remain outside this runner's exclusion; choosing a console
pair above the default scan range avoids that default scan, not every possible
custom scanner.

All subprocess groups, including commands that exit normally, retain their direct
leader until the last group signal. On Darwin, EPERM is accepted only with fresh
non-reaping exit observations around public libproc proof that the group is exactly
that retained zombie; a bounded read-only interval handles termination becoming
waitable just after the denied signal. No denied signal is repeated and other
permission errors, unknown/incomplete membership and lost ownership remain errors.
See Apple's [libproc implementation](https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/wrappers/libproc/libproc.c)
and the project-owned process-group tests. No enumerated PID is signalled.

An absolute runtime deadline covers fixture commands; cancellation or timeout does
not replay a mutation. SDK log draining uses nonblocking reads and a stop event,
so an inherited open pipe cannot hang teardown. A local `runtime-receipt.json`
records fixed phase/error categories and cleanup verification. A leftover listener,
SDK process, unresolved group error or log-drain failure prevents success. The
existing primary failure remains chained to cleanup errors. Logs/work data stay
private and no artifacts are uploaded. This candidate has host/mock test evidence;
no Darwin AVD boot, Android text mutation or OEM behavior is claimed yet.

Pure host checks (no SDK, ADB or AVD launch):

```sh
python3.14 -W error::ResourceWarning native-text/emulator/test_process_group.py
python3.14 -W error::ResourceWarning native-text/emulator/test_owned_runtime.py
python3.14 -W error::ResourceWarning native-text/emulator/test_runtime_environment.py
python3.14 -W error::ResourceWarning native-text/emulator/test_runtime_backend.py
python3.14 -W error::ResourceWarning native-text/emulator/test_proof_result.py
python3.14 -W error::ResourceWarning native-text/emulator/test_owned_errors.py
python3.14 -W error::ResourceWarning native-text/emulator/test_adb_diagnostic.py
```
