# Source-only owned-emulator runtime proof candidate

**The first reviewed workflow was published and stopped safely at its KVM access
preflight; no Android runtime ran. This software-backend correction awaits renewed
review before publication. All generated APKs are emulator-only test artifacts,
not user-installable releases. No user TV/Mac is
addressed, discovered, installed on, or controlled.**

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
