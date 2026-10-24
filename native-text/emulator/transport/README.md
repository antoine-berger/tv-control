# Lab transport implementation: pure core only

The implementation is incomplete: there are no sockets, lab entrypoint,
manifest/build wiring, APKs, ADB transport or runtime capability. It is not usable
as a typing transport and is not part of a release. The independently reviewed
[design](../LAB_TRANSPORT_DESIGN.md) remains the integration contract.

* LabProtocol parses complete bounded binary frames with closed commands/reasons,
  canonical UTF8, immutable decoded values and per-connection input budgets.
  Future I/O must call checkedBodyLength before allocating a body, apply deadlines
  to every read/write and abort the operation on any codec fault.
* EditorBinding retains immutable, structurally validated identity/bounds/text/
  selection. This is not native-editor admission: the future UiAutomation issuer
  must still prove every password/hint/focus/action-capability/unique-tree gate.
  Observable empty text is distinct from null, which is refused.
* IssuedEditor owns one in-memory ref/epoch, original monotone expiry, single
  consume before entry, and a terminal outcome available only after cleanup.
  Entry and consume never dispatch anything; the future common wrapper must mark
  entry immediately before its one real engine call. Invalid/cancelled/lost state
  after possible entry cannot be cleared by a later verified result. Ref bytes,
  text and pending payload are excluded from object string representations.
* The real common NativeTextInstrumentation.onStart now has one narrow ingress
  hook and a NativePendingRequest owner created before context/connection/ingress.
  It calls the engine at most once, closes its registered lifecycle in finally,
  and emits only fixed Bundle metadata after explicit remote cleanup attestation.
  The lab socket subclass does not exist yet. The default legacy research ingress
  still yields NOT_APPLICABLE cleanup and can never qualify as lab transport.

Pure host checks compile/run these actual Java classes with the pre-approved host
JDK, fake clocks/entropy and synthetic strings. No Android API, SDK executable,
network/socket, emulator or TV is involved:

```sh
python3.14 native-text/emulator/transport/test_core.py --java "$JAVA_HOME/bin/java"
python3.14 native-text/emulator/transport/test_wrapper.py --java "$JAVA_HOME/bin/java"
```

The checks cover malformed/truncated/noncanonical frames and bounds, nonce copies,
expiry and overflow, no expiry renewal at consume, cancellation before/after entry,
uncertain result/cleanup/lost callback, replay, two isolated invocations and racing
consumers. They do not prove UID/SELinux/FD/IPC/wrapper behavior. Those subsequent
lots need independent code review and a separate root-controlled Android build/run.
The wrapper test compiles and executes the actual common onStart/owner against
Android and engine fakes, never a subclass replacing onStart. It covers ingress/
connection/engine/result/close/proof/publication failures and an adapter to the
actual synchronized reference state for cancellation before/after its entry
boundary. This is lifecycle contract evidence, not Android lifecycle execution.
The [host/Java interface](CONTRACT.md) separates final Bundle cleanup from the
additional required remote shell-owner wait proof for app_process. Local exit0
and a client CHANNEL_CLOSED event are never enough; missing remote proof means
unknown outcome/cleanup unverified. Future IPC borrows socket FDs without closing
them through Os; the socket object remains the sole close owner.
