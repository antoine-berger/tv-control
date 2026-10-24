# Lab host supervision: fake-process qualification only

The host modules have no CLI, ADB discovery, SDK startup, production capability or
MCP integration. No Android launcher is selected by default. Tests use only pure
clocks/channels or directly owned Python subprocesses as stand-ins. Their synthetic
Bundle/owner markers do not qualify Android socket policy, peer identity, Binder
cleanup, the actual wrapper or the remote shell owner.

The wire/Bundle contract comes from wrapper source
`f8a70007d342d59d40739171b7d77758182c2af1`; the reviewed remote owner source is
`9bf44411615ebb719afc2fff68148e1545470d57`. A future owned-AVD launcher must verify
its exact installed artifact/source hashes, package/signing/user/UID identity and
private ADB ownership. Neither source existence nor these host tests establish
that an owner/client is installed or authorized to run. Integration and a new
root-controlled runtime GO remain separate gates. A physical-TV experiment is
not authorized by this work.

## Modules and injected runtime boundary

* `transport_wire.py`: bounded private command frames and incremental response
  parsing. Append is bounded before encoding; private append/ref representations
  expose no data. Device expiry is returned unchanged and never converted or
  renewed by the host.
* `transport_bundle.py`: a qualifying session needs one correlated READY STATUS
  group and one FINAL RESULT group, with final code0. An early refusal before
  entry may omit READY but cannot qualify a transport session. Progress is never
  a terminal result. Entry needs
  preceding READY; cleanup failure or a transport failure cannot become refusal
  after possible entry. Legacy output is not lab qualification.
* `transport_pipes.py`: private binary pipes, short nonblocking writes/reads and
  direct-child/process-group ownership. Call `PipedChild.require_supported()`
  before spawn; launch in a new session. Its registration callback receives the
  child before pipe setup can fail. Generic subprocess logging/temporary-output
  capture and `communicate(input)` are deliberately absent.
* `transport_host.py`: one operation per `Host`, with a single absolute host
  deadline at most15s. It receives the existing owned-runtime launcher and guard;
  it does not accept a serial or arbitrary command argv. The launcher must retain
  every started channel in `owned()`, including failed partial startup. Guard
  checks are required before instrumentation, client launch and COMMIT, and after
  completion. The launcher/guard implementation must itself be bounded.

The launcher's `instrumentation(instance)` and `client(instance)` methods must
construct only the reviewed fixed commands through the existing private ADB
server/serial and non-PTY `shell -T`, with stdin enabled (never `-n`). The client
command is the pinned owner script with instance, pinned client artifact and
fresh expected helper UID. No payload is a shell command or command argument.
The public32-hex instance is launch
metadata, never a ref/token. Only non-secret artifact paths, class/component names,
instance and freshly pinned UID can appear in argv/environment. Payload, epoch
and ref stay in memory and binary stdin/stdout pipes. No raw stderr is logged;
unexpected stderr is a fixed failure category. Python memory is not promised
securely erased.

The client is launched only after correlated READY. stdin stays open after
COMMIT, including after an early final server Bundle. Normal closure occurs only
after the remote-owner exit frame. Explicit cancellation instead closes stdin
with EOF semantics, discarding any incomplete pending COMMIT rather than inserting
a CANCEL into its body. Cancellation/loss/malformed data remains sticky and never
creates a new invocation or COMMIT retry.

## Independent proof obligations

The server's post-cleanup final Bundle and the remote owner's post-wait/reap
marker are separate, indispensable proofs. `CHANNEL_CLOSED`, local adb/am exit0,
local kill/wait and a final server Bundle alone prove neither client reaping nor
both remote cleanup obligations. Missing/malformed evidence yields UNKNOWN and
unverified aggregate cleanup. Fixed per-side evidence is retained separately;
only a complete, non-uncertain operation can set `qualified`.

Local group cleanup happens before reaping its held leader and is containment
only. It never substitutes for any remote attestation. There is no filesystem,
TCP/forwarding, permission, debug, force-stop or reconnect fallback. Failure stops
the scenario; a later Bundle cannot clear an earlier host uncertainty.

## Host-only checks

```sh
python3.14 -B -W error::ResourceWarning -m unittest discover \
  -s native-text/emulator -p 'test_transport_*.py' -v
```

These checks cover frame boundaries/order/truncation, strict UTF8/UTF16 bounds,
private representations, stale/duplicate/progress/legacy Bundles, exact cleanup
and entry tuples, short writes, cancellation, no replay, guard failure, setup and
cleanup exceptions, exit0 without either proof, a surviving fake client, slow or
oversized output and a private stderr canary. Real stand-in children are reaped
and their pipes closed. No ADB, SDK, Java/Android process, emulator, VM, TV or
network service is invoked.
