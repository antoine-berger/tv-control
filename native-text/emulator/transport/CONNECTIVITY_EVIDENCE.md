# API31 lab connectivity: blocked, not qualified

The observed source was `0db5cfb749f4eb5c1b8f7e3e4c9c70764b271139` on
2026-10-24. The run used an isolated AOSP API31 ARM64 AVD on an ARM64 macOS host
with HVF, no window and no audio. Its ADB server, homes, ports and test packages
were owned by the lab runner. No physical TV was addressed.

## Build and review scope

The source compiled against the real public API31 `android.jar`, passed D8 with
min-api31, and produced a locally signed APK. The builder checked its signature
and local artifact pins and removed the disposable signing key before emitting
the build receipt. The exact source received independent review.

Host checks passed: 193 socket assertions, 80 contextual native-error assertions,
299 client assertions and 1,735 diagnostic assertions. The independent review
also passed 83 additional assertions. These checks qualify their tested
contracts; they do not establish Android connectivity or text insertion.

The earlier [engine-only evidence](../MAC_RUNTIME_EVIDENCE.md) exercised the
shared engine through a different entrypoint. It is not evidence that this host
transport or its instrumentation ingress can insert text.

## Executed scenario

Only `lab-connectivity` was selected. The host queued an initial CANCEL, never
OBSERVE or COMMIT. No editor was captured, no text was submitted, and the server
reported that the engine was not entered.

| Observation | Result |
| --- | --- |
| Overall host outcome | `UNKNOWN / STDERR_OUTPUT`, unqualified |
| Client diagnostic | Valid closed metadata: `CONNECT / POLICY_DENIED` |
| Client diagnostic cleanup | `VERIFIED`, a client self-report only |
| Server READY | Observed by the host |
| Server final outcome | `refused / DEADLINE`, `entered=NO` |
| Server post-cleanup Bundle | `cleanup=VERIFIED` |
| Client protocol termination | No `CHANNEL_CLOSED`; remote reaping proof not validated |
| Response parser | 27 bytes, OWNER-shaped frame rejected at `ORDER_OR_FIELDS` |
| Staging and local cleanup | Verified |
| SDK containment | Verified separately; owned SDK processes gone and ports closed |

Neither the client diagnostic nor successful VM teardown replaces the missing
client terminal protocol proof. The aggregate transport cleanup remains
unverified even though SDK containment succeeded.

## Measured timing and its limits

All values below are local monotone host measurements. READY means receipt by
the host, not a timestamp from Android.

| Segment | Milliseconds |
| --- | ---: |
| READY received to client launch call returning | 3,109 |
| Host pre-client guard | 1,593 |
| Launcher client guard | 1,512 |
| Popen beginning to child registration | 3 |

An earlier measured run at source
`3edf44648de16483ac2c2dbbbbd1084240c87e76` took 3,255 ms from READY receipt to
the client call returning. Neither measurement establishes a host-side overrun
of the nominal 5,000 ms server accept window. Remote shell and `app_process`
startup were not measured. A deterministic test demonstrates that injected
costs can exceed that window; those injected costs are not measured run latency.
No guard, command, route or deadline was changed to hide the failure.

## Permission signal and stop decision

The native bind/connect API may throw an exact `IOException` without a structured
errno cause. The reviewed classifier uses typed errno first. Only at those two
immediate catches, an exact cause-free `IOException` message equal to the local
public `Os.strerror(EACCES)` or `Os.strerror(EPERM)` produces the existing
`POLICY_DENIED` stop signal. No message, nonce, payload or private path is retained
or published. Unknown and non-matching errors never establish permission.

This is a conservative denial-compatible diagnostic, not proof of the exact
numeric errno or of SELinux causation. Effective SELinux labels, the compiled
policy and a correlated AVC were not collected. Static policy inspection alone
also cannot establish the effective run policy. See the source references and
classification limits in [DIAGNOSTICS.md](DIAGNOSTICS.md).

The positive stop signal ended this route. No further probe, alternate transport,
permission grant, security change or retry was attempted after it. This source
does not qualify the host-to-instrumentation transport or integrated text entry.
There was no editor/text scenario, no helper installation on a TV, and no TV
wake, playback, audio, source or Jellyfin-history operation. The prototype is not
part of the v0.4.11 release.
