# Lab transport implementation: host-tested components

Java ingress, IPC client and a separate API31 lab manifest/builder now exist as
host-tested source. The full host integration is a separate reviewed lot. There
are no transport APKs or runtime qualification yet. It is not usable as a
qualified typing transport and is not part of a release. The independently reviewed
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
  The final lab socket subclass inherits the actual common onStart and finish;
  host tests inspect its compiled method declarations and exercise that entrypoint.
  The base remains overridable for the historical engine-only RuntimeProof; this
  is a tested lab-class invariant, not a Java prohibition for every subclass.
  The default legacy research ingress
  still yields NOT_APPLICABLE cleanup and can never qualify as lab transport.
* LabEditorIssuer observes a native editor from that same UiAutomation connection,
  checks intended package/resource before text access, rejects hint/secret metadata,
  and requires the shared unique-focus predicate then refreshes and rechecks all
  admission gates. The original observation time and exact immutable binding are
  retained; it performs no action or selection repair. Its host tests use Android
  and unique-focus fakes, not a real AOSP node or an independent tree algorithm proof.
* LabPrivateRequest uses an exclusive private scratch directory/request/cancel
  under the wrapper's Context cache. It serializes the existing engine frame with
  the original expiry, retains handles and captures pathname identity baselines
  **after creation**. It refuses observed links/collisions and later baseline
  mismatches; it does not atomically authenticate a newly created object from
  its retained handle. Unknown allocation or close failures keep cleanup
  unverified without another destructive attempt. Pure Java cannot
  authenticate an arbitrary File as an Android sandbox; the Context is that trust
  source. cleanupVerified is conditional on the exclusive unaltered lab assumed
  by the design; it does not detect or prove that assumption. Hostile same-UID
  writers are outside that model, including the windows described below.

Pure host checks compile/run these actual Java classes with the pre-approved host
JDK, fake clocks/entropy and synthetic strings. No Android API, SDK executable,
network/socket, emulator or TV is involved:

```sh
python3.14 native-text/emulator/transport/test_core.py --java "$JAVA_HOME/bin/java"
python3.14 native-text/emulator/transport/test_wrapper.py --java "$JAVA_HOME/bin/java"
python3.14 -W error::ResourceWarning native-text/emulator/transport/test_owner.py
python3.14 native-text/emulator/transport/test_issuer.py --java "$JAVA_HOME/bin/java"
python3.14 native-text/emulator/transport/test_private_request.py --java "$JAVA_HOME/bin/java"
python3.14 native-text/emulator/transport/test_socket_io.py --java "$JAVA_HOME/bin/java"
python3.14 native-text/emulator/transport/test_session.py --java "$JAVA_HOME/bin/java"
python3.14 native-text/emulator/transport/test_lab_entry.py --java "$JAVA_HOME/bin/java"
python3.14 native-text/emulator/transport/test_socket_client.py --java "$JAVA_HOME/bin/java"
python3.14 -W error::ResourceWarning native-text/emulator/transport/test_build_lab.py
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
The client-owner.sh source preserves stdin in a foreground child and emits a
binary remote-exit witness after that child's return. Actual shell host tests with
owned Python fakes check the CLOSED-before-exit distinction, nonzero/signalled
child outcomes, EOF misuse and rejected public arguments. No Android tool runs.
Private-cache tests use real host files and the exact extracted TextEngine.Request
decoder. They exercise exclusive creation/modes, original expiry, engine unlink,
observed post-baseline symlink/inode replacement preservation, unknown-allocation checkpoints and close
failures. They do not qualify the Android NIO provider or a device cache.

### Cache baseline limitation: three explicit regression witnesses

DIRECTORY_CREATED, REQUEST_CREATED and CANCEL_CREATED checkpoints stay immediately
after their respective create call, before the first pathname attributes. A
same-UID actor in any of those windows can move the original and substitute an
object with matching type/owner/modes. The subsequent baseline can adopt that
replacement and cleanup can delete it and return true. In the file cases, the
retained FileChannel can still refer to the moved original while the pathname
baseline refers to the replacement. There is also no atomic check→unlink guard
against a same-UID writer during cleanup.

Three real-host-filesystem regressions deliberately violate the exclusive-lab
assumption and record this limitation. They retain different original/replacement
file keys, moved originals and hardlink witnesses, including proof that the
request payload went to the original channel while the foreign path was adopted.
Those checks expect the existing conditional true result; they are **not security
passes** or evidence that interference is detected. Normal, unaltered-lab cleanup
tests and post-baseline foreign/symlink refusal tests remain separate. No checkpoint
is moved and no private descriptor/reflection/permission route is introduced.

The socket layer now uses public LocalServerSocket/LocalSocket and borrowed Os
nonblocking I/O. A failed server constructor keeps allocation/cleanup unverified.
The client binds a public abstract address before setting NONBLOCK and making
exactly one connect call; any denial or pending-connect error stops. Host fakes
cover deadlines, peer admission before private I/O, partial frames, close failures
and descriptor reuse. They do not qualify JNI, SELinux or Android FD closure.

LabSocketSession registers before opening resources, permits a first CANCEL with
zero editor observation, and otherwise issues a ref only for the owned synthetic
fixture package/resource. Its watcher uses the same synchronized core cancellation/
entry boundary; every codec failure aborts, and late failures stay uncertain.
The final Bundle is emitted only after bounded watcher join and attempted socket/
cache closure with explicit results. The cache part is conditional on the trusted
Context and exclusive unaltered-lab precondition above, not proof of that condition.
The Bundle attests those resource results within that boundary;
the host must separately require the remote shell's client wait/reap witness.
Tests use fake ports/cache/editor/engine and do not qualify a remote lifecycle.

The Java client authenticates the kernel peer UID before reading stdin. It keeps
stdin open after COMMIT and treats EOF as cancellation. Only bounded binary REF/
CHANNEL_CLOSED frames reach stdout. A stopped partial frame or a decoded late
invalid/cancelled frame cannot silently become successful closure. The client
joins its stdin worker and closes its socket before emitting CHANNEL_CLOSED;
an unresolved worker yields a nonzero exit, never verified cleanup. Borrowed
stdio is not closed. A child exit still requires the separate owner wait witness.

## Unexecuted API31 lab build source

LabManifest.xml contains only the self-targeted LabSocketInstrumentation and its
application: no activity/service/provider/receiver, uses-permission, debuggable
flag or exported typing endpoint. The target is only the disposable fixture.
build_lab.py is Darwin ARM64 lab tooling, not an installer. Before its first tool
call it checks the existing private SDK/baseline-build receipt, all recorded SDK
hashes, the approved private Java archive, API31/build-tools coverage and a clean
checkout at the explicitly reviewed commit. It performs no download or license
acceptance and never invokes adb/emulator. The baseline build is checked only as
existing provenance; it is not silently replaced by this distinct lab artifact.

The separate lab-helper.apk includes the client and wrapper. Its single signing
certificate must match the locally generated disposable certificate, verified
through apksigner; the private test key is removed before the new receipt is
written. lab-build-receipt.json binds schema1, lab_package,
instrumentation_component, source_commit, sdk_provenance_sha256, lab_apk_sha256,
lab_certificate_sha256 and owner_sha256. The certificate is never learned from
the device. APK/signing/owner pins remain an additional gate for later host
staging/PM reads, not permission to install on a TV. Builder host tests mock every
executable call; Android compilation and signing remain unexecuted until the
root-controlled review/build gate.
