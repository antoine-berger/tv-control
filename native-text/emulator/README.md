# Source-only owned-emulator runtime proof candidate

**No workflow publication or Android runtime run has occurred. All generated APKs
are emulator-only test artifacts, not user-installable releases. Parent review is
required before deliberately pushing `qa/native-text-runtime`. No user TV/Mac is
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

KVM is used only when already accessible to the hosted runner. Otherwise the job
fails clearly before provisioning; no ACL/group/permission changes or sudo are
performed. Emulator
has no window/audio/metrics, a fresh wiped AVD, 240s boot deadline, 30s instrument
bounds and a 25-minute total job deadline. Commands are captured to bounded
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
