# Owned AOSP API31 runtime evidence on macOS

The reviewed prototype completed its owned-emulator matrix on 2026-10-24;
the result was recorded at **17:52:09 UTC**. The tested source is
`59e2de170d08f30c8eb26828ec9eee1be33420b7`, using the private lab's build-3/run-5.
This document records that run; it does not report another execution.

The result covers one disposable AOSP/default API31 `arm64-v8a` emulator on a
native ARM64 Mac with HVF acceleration enabled. It establishes exact append to
known accessible text and the listed refusals. It does not qualify arbitrary
empty fields, third-party apps, an OEM TV, or a shipping host transport. The
prototype is excluded from v0.4.11 and remains on a separate QA branch.

## Passed cases

| Case | Observed proof result |
| --- | --- |
| Standalone helper without token | refused |
| Synthetic keyevent layout mismatch | exact expected remap |
| Normal field seeded with public `seed`, selection at end | exact Unicode/percent/quote append verified |
| Empty field without hint, accessible text null | refused; null remained unchanged |
| Hint, password, visible-password | refused; unchanged |
| Middle selection | refused; unchanged |
| Focus switched to the second public fixture editor | refused; both editors unchanged |
| Stale window identity | refused; unchanged |
| Cancellation marker | refused; unchanged |

The request was consumed and replay refused for every applicable engine case.
Both the normal result and post-replay text were checked. The focus case freshly
reacquired/refreshed the second editor for its post-invocation and post-replay
comparisons; checking only the original editor was not accepted as proof.
Default IME, selected subtype, enabled accessibility services and accessibility
enabled secure-setting snapshots equalled their initial values after each case.

Null accessibility text stays opaque: the engine continues to refuse it rather
than treating it as an observed empty string. The normal case begins with a known
public seed; its success is an append-only proof within that exact contract.
The shared TextEngine was unchanged by the runner/diagnostic/fixture corrections;
the recorded helper/engine DEX comparison was unchanged. No deadline, identity,
permission or cancellation check was relaxed to pass this run.

## Exact build-3 artifacts

These SHA256 values match the build receipt and the recorded successful result.
They identify disposable emulator test APKs, not downloadable releases.

| Artifact | SHA256 |
| --- | --- |
| fixture.apk | `c95d89563a72def3de47949ab4278f75cc1f6421805d50bb1a334cebba05b040` |
| helper.apk | `642d775ba6741ae48b38bf6a0c9ff276220c07be7da6df6e0a0466ee772c74f1` |
| proof.apk | `f7c21d6a32c445bcf4f664041f14b9422685c00bd5d2b950bf44b8d3f37f2e1c` |

The helper is the standalone no-activity/no-service/no-permissions APK with
self-targeting instrumentation. The proof replaces it only inside the disposable
AVD and creates its request in its own private cache. Ordinary host shell access
to that cache is not implemented as a production transport. Disposable APK-signing
keys were removed by the builder; signed APK hashes vary across separate builds.

## Environment and provenance

The explicitly approved private lab used Google command-line tools 23.0,
platform-tools 37.0.1, emulator 37.2.12, Build Tools 35.0.0 and Platform 28 for
compilation, with the AOSP/default API31 ARM64 system image. Java was the isolated
Temurin 21.0.13+9-LTS macOS aarch64 archive, not a system installer or Rosetta.
Runtime preflight rechecked the local provisioning/build receipts and file hashes,
native architecture, existing stable license, API/ABI and HVF support.

Google archive sizes and published SHA1 checksums were verified against official
repository metadata; local SHA256 and installed-file hashes were recorded
separately. The local SHA256 values are not claimed as Google-published SHA256
pins. The Temurin archive matched its publisher-provided SHA256; signature
verification was not performed. Exact image provenance preserved an upstream
metadata discrepancy: repository/package.xml revision 4 versus the verified
`arm64-v8a-31_r03.zip` contents' source.properties revision 3. API31/default/ARM64
and the archive checksum matched; neither artifact nor metadata was rewritten.

The user approved the Android SDK terms and Temurin licensing for this isolated
lab. The official provisioning CLI wrote the stable `android-sdk-license` during
installation of those expressly approved packages; no `yes` automation or manual
license hash was used. The runner itself only checks existing acceptance and does
not accept terms. Preview packages and analytics opt-in were absent. Temurin's
GPLv2 with Classpath Exception, Assembly Exception and bundled notices were kept.

The AVD used 1536 MiB RAM, two cores and 480x800 display with headless/no-audio,
no-snapshot/wiped-data, camera-disabled and no-metrics options. HVF was required;
there was no software or Rosetta fallback. A directly owned foreground ADB server
used a private loopback port and fresh emulator port pair, private homes/keys and
an exact emulator serial. Ownership, loopback and target-list guards passed before
fixture installation/input. No production TV commands were dispatched.

## Cleanup and evidence limits

The final receipt records `proof_completed=true`, `cleanup_verified=true` and no
cleanup failures. Fresh postchecks recorded zero private SDK processes; the
existing TV ADB identity was unchanged. Cleanup is specific to this owned run,
not a claim that every possible external ADB scanner/process is controlled.
The existing client check/use autostart race and nonatomic Android accessibility
check/action boundary remain documented limitations.

Private local evidence retained by the operator comprises
`qa-tv-development-20261024/native_text_runtime_result.json`,
`android-emulator-lab/build-3/build-receipt.json`, and
`android-emulator-lab/run-5/runtime-receipt.json`, plus provisioning/review and
postcheck records. Only fixed outcomes and nonsecret hashes are reproduced here.
SDK logs, UI text, tokens, keys, screenshots and private lab artifacts are not
included or uploaded. This documentation update adds no SDK/AVD execution,
physical-TV qualification, helper installation on the TV, or new persistent access.
