# Lab host / Java interface v1

Frozen for parallel implementation review. Java IPC/client/entrypoint and the
remote-owner source now have host/fake tests; the whole transport has not been
compiled or qualified on Android. Host integration remains a separate lot. All channels belong to the
same exclusively operated disposable AVD; same-UID impersonation is not prevented.

## Final Instrumentation Bundle

Only `INSTRUMENTATION_RESULT` fields after the common wrapper's cleanup are
authoritative. Ignore/reject attempts to substitute `INSTRUMENTATION_STATUS`
progress, stdout acknowledgements or local exit0. Strict lab parsing requires
exactly one of each closed field below, with no duplicates/unknown fields:

| Key | Values |
| --- | --- |
| tv_native_lab_protocol_v1 | `1` |
| tv_native_lab_instance_v1 | canonical32 lowercase hex chars; public rendezvous identifier |
| tv_native_text_v1 | `refused`, `uncertain`, `verified` |
| tv_native_cleanup_v1 | `NOT_APPLICABLE`, `VERIFIED`, `UNVERIFIED` |
| tv_native_entered_v1 | `NO`, `YES` |
| tv_native_reason_v1 | names in LabProtocol.Reason / NativePendingRequest.Reason |

The instance is public launch metadata, not a payload/ref/cache token. Its fixed
socket name is `tv.native.lab.` followed by the instance. No text, before/after,
ref, epoch or private cache token enters argv/environment/status/logs. Legacy
default wrapper output has no lab instance and cleanup NOT_APPLICABLE; it cannot
qualify as lab transport. Verified requires entered YES and reason NONE.
The common owner records device uptime before context/UiAutomation connection;
the lab instance deadline must derive from that timestamp, not restart after a
slow connection. Reference expiry remains separately anchored to observation.

Client connection uses the reviewed public pre-bind design: assign a LocalSocket
owner, mark openAttempt, bind `tv.native.lab.client.<instance>` in ABSTRACT without
listen, preserve flags via F_GETFL|O_NONBLOCK and verify them, then call exactly
one connect(endpoint). Any failure, including EAGAIN/EINPROGRESS or policy denial,
aborts and closes the same owner without another route/retry. Peer UID is checked
before payload. Never use the timeout overload or hidden FD factories. A socket
never opened must not be closed through LocalSocket.close, which lazily creates
an FD; attempted/unknown allocation remains cleanup unverified.
This public pre-bind path follows API31 [LocalSocket](https://android.googlesource.com/platform/frameworks/base/+/refs/tags/android-12.0.0_r1/core/java/android/net/LocalSocket.java),
[LocalSocketImpl JNI](https://android.googlesource.com/platform/frameworks/base/+/refs/tags/android-12.0.0_r1/core/jni/android_net_LocalSocketImpl.cpp)
and [cutils single-connect](https://android.googlesource.com/platform/system/core/+/refs/tags/android-12.0.0_r1/libcutils/socket_local_client_unix.cpp)
source. It is not a SELinux workaround: a denied bind/connect ends the experiment.

The Bundle cleanup field attests the **server wrapper's** owned watcher/FD/cache
cleanup before finish within the exclusive, unaltered-lab model. Its cache result
is conditional on a trusted Context cache and no same-UID alteration; it does not
detect or prove that precondition. Path identity baselines are read AFTER creation,
not atomically authenticated from a created FD. DIRECTORY_CREATED, REQUEST_CREATED
and CANCEL_CREATED are three unprotected create→baseline windows: replacement
with matching metadata may be adopted/deleted with cleanupVerified=true, while a
retained file channel still points to the moved original. Check→unlink is also
not atomic against a same-UID writer. The explicit host regression witnesses
document that out-of-model limit, not successful interference detection.
It does not prove the separately running client died.
Host qualification additionally requires the exact remote client-owner proof
below, then local owned-process cleanup. Missing any proof means uncertain plus
cleanup unverified; stopping adb/am or an exit0 never substitutes for either proof.

Startup control is separate: after binding/listening and configuring bounded
nonblocking I/O, the instrumentation sends exactly one public sendStatus code1
group with tv_native_lab_ready_v1=READY, tv_native_lab_protocol_v1=1 and the same
tv_native_lab_instance_v1. The host parses that complete STATUS group separately
and launches the client only after exact correlation. No probe connection may
consume the single peer slot, and no arbitrary sleep establishes readiness.
READY is not terminal/effect/cleanup proof; only final RESULT fields qualify that.

## Binary private channels

All frames: unsigned32 big-endian body length, magic `0x4e544c31`, unsigned8
version1, unsigned8 opcode, then the exact fixed fields. Total frame≤16384 bytes.
No JSON, optional fields, padding, trailing bytes or EOF-based frame delimiter.
Input budget≤32768 bytes and6 frames per connection. REF and request payload are
private pipe data; they must never be printed or captured by generic subprocess
logging. Reject noncanonical UTF8 and append length>256 UTF16 units /2048 bytes.

| Opcode | Direction | Exact body after magic/version/opcode |
| --- | --- | --- |
| 0x01 OBSERVE | host stdin → client → server | zero bytes |
| 0x02 COMMIT | host stdin → client → server | epoch16 bytes, ref16 bytes, u32 UTF8 length, exact append bytes |
| 0x03 CANCEL | host stdin → client → server | zero bytes |
| 0x81 REF | server → client stdout → host private parser | epoch16, ref16, signed64 positive absolute device-uptime expiry |
| 0x82 CHANNEL_CLOSED | Java client stdout → host | u8 value1 only |
| 0x83 REMOTE_CLIENT_EXIT | remote shell owner stdout → host | public instance16 bytes, u8 exit class0(normal) or1(nonzero/signalled) |

REF has body length46, total50 bytes. CHANNEL_CLOSED has body length7, total11.
REMOTE_CLIENT_EXIT has body length23, total27. REF contains no UI text or identity;
its opaque epoch/ref differ from the public instance. The host returns the exact
private epoch/ref in COMMIT and never computes or extends device expiry.

There is at most one REF, then one CHANNEL_CLOSED, then one REMOTE_CLIENT_EXIT.
An early supported refusal may produce no REF. Duplicate/unexpected response
order, unknown opcodes, truncated/malformed frames, extra data or mismatched
instance makes the operation uncertain and cleanup unverified. Strictly decode
the private stream; never scan for a magic marker through arbitrary diagnostics.
The remote shell owner emits0x83 only **after waiting/reaping its direct owned
app_process child**. CHANNEL_CLOSED is emitted by a client that is still alive;
it proves neither client termination nor server effect. The client exits bounded
after that event and never waits for the Instrumentation Bundle. Only the host
correlates that separate process's final Bundle.

The remote shell owner source is client-owner.sh; its exact wait/marker command
needs independent review and Android execution remains unverified:
no broad process search/kill, daemon, new permissions or ownership inferred from
a recycled PID. The public instance and fixed marker are not authentication
against another shell-UID process. The owner's pinned source/direct-child wait
and exclusive lab assumption are part of this proof boundary.
The owner takes exactly three non-sensitive arguments: public instance, pinned
client artifact path, expected helper UID. Its child command is
`CLASSPATH=<artifact> /system/bin/app_process /system/bin org.tvcontrol.nativeinput.LabSocketClient <instance> <uid> <&0`.
It runs in the foreground, records the child's status, emits0x83 using only fixed
octal bytes/public instance and then exits with the child status. No background
stdin rule, set-e, trap or PID kill is used. The host must still enforce deadlines;
a hung client yields no witness and remains cleanup unverified, never a local
process-exit-based substitute. Host shell tests substitute fixed executable paths
with owned fakes; they do not prove Android toybox/mksh execution or SELinux.

## EOF, cancellation and cleanup

The host must keep stdin open after COMMIT until the client exits or cancellation
is requested. `communicate(input=...)` followed by implicit stdin closure is not
this protocol: EOF means cancellation. Close/fail/cancel after possible entry is
sticky uncertain, never inferred refused or automatically retried. Late malformed
input after COMMIT also invalidates the operation; it cannot be promised absent
before dispatch. No operation follows terminal socket close.

Cancellation and markEngineEntered share IssuedEditor's synchronized boundary.
Before that transition cancellation prevents entry. After it, cancellation may
only request the existing private marker and preserve uncertainty; check/action
atomicity is not claimed. Common wrapper entered bookkeeping is observation of
that path, not a separate independently racing cancellation gate.

Socket/LocalServerSocket objects are sole FD-close owners. Os.poll/fcntl/read/write
borrow their FileDescriptor; never Os.close a borrowed FD and later socket.close.
Stop flags, bounded poll slices and joins must precede final owner close when
possible. A stopped watcher must not act on a reused numeric descriptor. Missing
remote cleanup/finish or an unresolved watcher is cleanup unverified and stops
the scenario. Full AVD teardown is a final containment step, not proof that the
wrapper/client cleanup contract passed.

Record allocation-attempted before calling LocalServerSocket(String). AOSP API31
does create→bind→listen without a constructor catch/close: a failure may occur
before any socket object is assigned to the caller. A null owner field therefore
does not prove no FD was allocated. This state is cleanup unverified; no finalizer,
alternate bind route or namespace/permission change may turn it into verified.
