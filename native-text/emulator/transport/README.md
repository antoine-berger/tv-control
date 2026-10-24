# Lab transport implementation: pure core only

This first implementation lot contains no sockets, Android imports, wrapper
changes, manifest/build wiring, APKs, ADB transport or runtime capability. It is
not usable as a typing transport and is not part of a release. The independently
reviewed [design](../LAB_TRANSPORT_DESIGN.md) remains the integration contract.

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

Pure host checks compile/run these actual Java classes with the pre-approved host
JDK, fake clocks/entropy and synthetic strings. No Android API, SDK executable,
network/socket, emulator or TV is involved:

```sh
python3.14 native-text/emulator/transport/test_core.py --java "$JAVA_HOME/bin/java"
```

The checks cover malformed/truncated/noncanonical frames and bounds, nonce copies,
expiry and overflow, no expiry renewal at consume, cancellation before/after entry,
uncertain result/cleanup/lost callback, replay, two isolated invocations and racing
consumers. They do not prove UID/SELinux/FD/IPC/wrapper behavior. Those subsequent
lots need independent code review and a separate root-controlled Android build/run.
