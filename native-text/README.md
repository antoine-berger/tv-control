# Native text insertion — isolated, non-shipping prototype

**Not enabled in TV Control, not published as a release, not deployed or run on a TV/Mac.**
This directory is outside the production workspace. The host crate has no concrete
ADB transport, capability probe, observation issuer, configuration option, or MCP
route. Nothing selects this implementation automatically. Do not merge it into a
release on the strength of compilation alone.

## Current review candidate: modern instrumentation, not legacy deployment

See [`emulator/README.md`](emulator/README.md) for the source-only proposed API31
runtime proof workflow. The guarded mutation/decoder logic is now shared in
`TextEngine.java`; the deprecated legacy class is a thin research-only wrapper.
Modern no-permission helper/proof APKs genuinely compile locally, but no Android
runtime has run yet. Modern Instrumentation requests the public
`FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES` and targets its own package. No legacy
runner is invoked by the modern proof. Installing any helper on a user device
still needs explicit authorization; there is no secure host transport yet.

The following legacy build/lifecycle notes remain research evidence, not a proposal
to ship or deploy the legacy wrapper. The standalone Rust seam is likewise an
old-runner contract prototype, not production routing or a modern capability issuer.
Shared protocol golden tests compare the entire Rust payload and actual Java
decoder, reject 150 malformed/truncated payloads, and test mid-transport cancellation,
timeout and errors conservatively. No framework-runtime claim follows from them.

## Purpose and boundaries

Android `input text` injects keys and may map `Tanya` to `Tqnyq` under a device IME.
This helper instead requests public `AccessibilityNodeInfo.ACTION_SET_TEXT` on a
strictly identified focused editor. It bypasses keyboard layout mapping; it does
not promise an app will honor the request or that accessibility text equals an
app's underlying model. Only exact **accessible text** readback is reported.

The helper is a legacy `UiAutomatorTestCase`, using a subclass to call the published
protected `UiObject.findAccessibilityNodeInfo`. It is compiled against Google's
SDK stub APIs, not platform implementation jars. No reflection, hidden API,
installed APK, replacement IME, locale/accessibility setting, credentials, daemon,
network listener or persistent access is introduced by project code. The legacy
runner itself has important lifecycle side effects; see the blocking audit below. Legacy `uiautomator runtest`
is deprecated; a successful SDK build does **not** establish that an OEM runtime
still supports it. AndroidX instrumentation is not silently substituted.

## Guards implemented

* API 26+ required for public `getHintText`/`isShowingHintText`; older systems refuse.
* Exact package, resource ID, class, window ID and bounds; focused, editable,
  enabled, visible, non-password node. Public input-type password variations
  (text/password/visible-password/web-password and numeric-password) also refuse,
  even if the accessibility password flag is false. Any hint metadata or missing
  text refuses.
* A bounded accessible-tree traversal requires exactly one focused node. Oversized
  or inaccessible child trees refuse (1024 nodes; 64 ancestor depth). The sole
  focused node must equal the selected node, not merely exist elsewhere. After
  traversal, the selected node is refreshed and its identity, pre-text, focus,
  password/hint guards, selection and action capability are rechecked immediately
  before dispatch. This cannot remove the final non-atomic IPC race.
* Exact pre-read text and collapsed selection at its UTF-16 end. Append semantics
  are implemented by setting `before + append`, never guessing selection position.
* UTF-8 strict decode, bounded fields/file, <=256 UTF-16 units; one action only.
* Request is unlinked before mutation. Expiry uses device uptime; a `cancel` marker
  and expiry are checked immediately before dispatch and after readback. No retry,
  fallback, compensation or auto-clear after uncertainty.
* Refreshes the original node and rechecks identity, unique focus and exact text.
  `performAction == true` or JUnit `OK` alone never means verified.
* Result is only `tv_native_text_v1 = refused | uncertain | verified`. No exception
  message, entered text, field contents or hierarchy is printed by project code.
* Standalone typed Rust seam requires explicit selection and same-session capability
  evidence, fresh observation, valid UTF-16 length, context deadline/cancellation.
  Transport failures after entry are uncertain. Payload types omit Debug/Display.

## Build proof and dependencies

Official repository metadata downloaded 2026-10-24 from:
`https://dl.google.com/android/repository/repository2-1.xml`

| Archive (official `https://dl.google.com/android/repository/` base) | Revision | Metadata SHA1 | Pinned SHA256 |
|---|---|---|---|
| `platform-28_r06.zip` | Platform 28 r6 | `9a4e52b1d55bd2e24216b150aafae2503d3efba6` | `8452dbbf9668a428abb243c4f02a943b7aa83af3cca627629a15c4c09f28e7bd` |
| `build-tools_r35_linux.zip` | Build Tools 35.0.0 | `2cfaa0bbb2336e9ec18ed3ecea84fa2e2af607bc` | `bd3a4966912eb8b30ed0d00b0cda6b6543b949d5ffe00bea54c04c81e1561d88` |

Both downloaded archives matched official metadata SHA1 before SHA256 pinning.
The offline build extracts only `android.jar`, `uiautomator.jar`, optional
`android.test.base.jar` (JUnit API), and Google's D8 compiler jar. None of the SDK
classes are bundled in the output. SDK dependencies remain external and subject
to their licenses. Build script performs no download or license acceptance.

Compiler: pre-existing OpenJDK `javac 21.0.12.1`, invoked through its module because
the environment has no javac launcher. The exact compiler version is checked.
Its JRE installation lacks `ct.sym`, so `--release 8` is unavailable; we use
`-source 8 -target 8 -bootclasspath <official android.jar>` instead. D8 min API=26.
JDK binary itself is the pre-existing environment dependency, not vendored or
cryptographically provisioned here; reproducing on another JDK requires review
and a new reproducibility proof rather than silently relaxing the version check.

```sh
python3 native-text/build.py \
  --platform /tmp/tv-native-sdk/platform.zip \
  --tools /tmp/tv-native-sdk/build-tools.zip \
  --java /usr/lib/jvm/default-java/bin/java \
  --output /tmp/tv-native-sdk/build1.jar
python3 native-text/tests/test_focus_guards.py --java /usr/lib/jvm/default-java/bin/java
cargo test --manifest-path native-text/host/Cargo.toml --offline
cargo clippy --manifest-path native-text/host/Cargo.toml --offline --all-targets -- -D warnings
```

Two clean Java + D8 builds produced byte-identical dexJARs (fixed ZIP metadata):
`16c6926d36dbcb533963f0382c74f430d6519e8ec104e2c5b31b65c6d4ba67ec`.
The local output is `/tmp/tv-native-sdk/build1.jar`; no binary is committed.
Ten standalone Rust tests pass; clippy with warnings denied and formatting pass.
Tampered SDK archive rejection was exercised before source compilation. Java compiles
against real SDK stubs. A source-extracted Java algorithm harness passes 21 checks
using fake nodes/input-type constants, including moved focus and post-traversal
refresh rejection. It tests actual guard method bodies but is not an Android
accessibility, Binder, OEM, lifecycle or integration test. **Java runtime behavior, OEM runner support and real editor
mutation have not been tested.** Host contract tests use a fake transport only.

## Required before any authorized runtime experiment or production integration

1. Parent review and explicit decision about deploying this new temporary helper.
   No experiment should submit a search, launch content, play audio or press volume.
2. Read-only capability/observation protocol must establish runner support, API,
   device uptime, unique editor/window identity, readable non-hint text and selection.
   XML dumps cannot supply or authenticate this full native observation. Capability
   flags in the test seam are supplied evidence, not a completed probe.
3. Implement a target/session-bound, issued-reference registry. Rust move semantics
   prevent accidental reuse of the same request value, not malicious reconstruction;
   the future adapter must consume issued refs before dispatch, even on uncertainty.
4. Own a cryptographically random exclusive `0700` directory named
   `/data/local/tmp/tv-native-text-<32 lowercase hex>`. Verify uploaded helper hash.
   Write bounded request as `0600`; only the token, never text, may appear in argv.
   Use binary stdin/file transfer, not interpolated shell text. Check filesystem
   ownership, regular-file/no-symlink properties and exact canonical owned paths.
   The Java canonical-path check is defense in depth, not a complete secure uploader.
5. Binary request is big-endian: magic `TVN1`, signed-range u64 device-uptime expiry,
   u32-byte-length UTF-8 package/resource/class, i32 window, four i32 bounds,
   length-prefixed before/after strings, EOF. No arbitrary command fields.
6. Concrete transport must enforce total context deadline and cancellation, bound
   output, accept exactly one status record, terminate its own helper process only,
   then clean up only its owned request/cancel/helper/directory on all paths. A lost
   connection may leave an owned artifact; report it and reconcile after reconnect,
   never broad-glob-delete other sessions. There is deliberately no uploader here.
7. Device calls can block inside Android, and focus/cancel checks cannot be atomic
   with mutation. Deadline/kill must be enforced externally; after dispatch timeout
   is uncertain, never safe to replay. App-provided accessibility metadata can be
   stale or transformed. No claim of arbitrary app compatibility is warranted.
8. Runtime fixture tests: ASCII, AZERTY/QWERTY independence, Unicode, percent/quotes,
   password/hint refusal, selection in middle, stale focus/window, duplicate focus,
   rejected/truncated action, cancellation at each boundary, timeout, output loss,
   repeat token, and cleanup. Verify no state/settings/APK remain afterward.

Public API reference:
https://developer.android.com/reference/android/view/accessibility/AccessibilityNodeInfo

## Blocking legacy-runner lifecycle audit

The runner is **not side-effect free**, even when the test only uses public APIs:

* API31 `UiAutomatorTestCase.setUp` switches the default IME and subtype when
  `disable_ime` is exactly `true`; its restoration is TODO. The helper overrides
  setup to require **explicit string `false` before calling super**, rejecting both
  missing and true values. Future transport must hardcode `-e disable_ime false`
  and must not accept a user-supplied runner-parameter map.
* The default legacy runner calls `setRunAsMonkey(false)`, which clears the existing
  ActivityManager activity controller without saving/restoring it. There is no
  demonstrated public flag that avoids this. A helper-level setup check happens
  too late to prevent that runner side effect.
* Runner/dump automation connections can temporarily suppress other accessibility
  services. Do not overlap with another automation/accessibility session.
* Therefore deployment is blocked pending an auditable, read-only way to establish
  no existing activity controller, no conflicting automation, and acceptable
  accessibility interaction. SDK level and `uiautomator help` do not prove safety.
  The host seam requires separate `lifecycle_preflight_safe` evidence, which the
  fake tests supply but no real probe currently produces. These plain boolean
  fields are a proposed contract, not trusted capability or authorization tokens. If safe preconditions
  cannot be established without hidden APIs/settings changes, do not deploy this
  approach. Never infer authorization from successful compilation.

Audited sources supplied by the research task:
* Published legacy API signatures:
  https://android.googlesource.com/platform/frameworks/base/+/df588a22929faed4795c97ee3eebfa32bb6186fc/cmds/uiautomator/api/current.txt
* API31 base test setup and IME switch implementation:
  https://android.googlesource.com/platform/prebuilts/fullsdk/sources/android-31/+/refs/heads/main/com/android/uiautomator/testrunner/UiAutomatorTestCase.java
