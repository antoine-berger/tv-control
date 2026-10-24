# Private wrapper transport: lab design for review

Status: **design only; no implementation or runtime approval**. Base source:
`71fa700a9629aa513f15098b95820845505ef5c0`; previously tested engine/fixture source:
`59e2de170d08f30c8eb26828ec9eee1be33420b7`. No SDK, AVD, TV or helper execution
was performed for this design. Nothing enters v0.4.11 or a production transport.

## Proof boundary

Run5 proved the shared TextEngine on one owned AOSP API31 ARM64/HVF AVD, including
known-text append, null/sensitive/hint/focus/selection/window/cancellation refusals,
consumption and replay refusal. RuntimeProof created the request in its own cache
and called TextEngine directly. It did not prove delivery from host stdin through
the public Instrumentation wrapper. This proposal adds that separate lab proof;
it does not authorize installing a helper or client on a physical TV.

The future positive path must enter the public self-targeting Instrumentation via
`am instrument`, obtain its normal UiAutomation connection, receive a private
request and call TextEngine exactly once through the common wrapper path. No
test-only direct TextEngine invocation counts as wrapper/transport evidence.
The engine, native selector checks and four-second request lifetime stay unchanged.

## Public API and client assessment

* [LocalServerSocket(String)](https://developer.android.com/reference/android/net/LocalServerSocket)
  binds in the Linux abstract namespace. [LocalSocket](https://developer.android.com/reference/android/net/LocalSocket)
  exposes abstract addresses, streams and kernel peer credentials; these are
  public APIs. LocalServerSocket.accept has no public timeout parameter.
* The [API31 toybox netcat source](https://android.googlesource.com/platform/external/toybox/+/refs/tags/android-12.0.0_r1/toys/net/netcat.c)
  advertises `-U`, stdin/stdout and timeouts. Its `usock` copies the name with
  strcpy into sun_path and passes a full sockaddr_un; it does not translate an
  `@name` into a leading NUL. Thus stock `nc -U` is not a client for the abstract
  LocalServerSocket name. No live nc command was run. A filesystem socket is not
  an alternative: no shared writable directory, chmod/grant or cache bypass is
  proposed to make shell access an app-private path.
* Candidate client: one short-lived Java main class, included in the reviewed
  lab APK/JAR only, launched as the existing shell UID with standard app_process.
  [AOSP app_process](https://android.googlesource.com/platform/frameworks/base/+/refs/tags/android-12.0.0_r1/cmds/app_process/app_main.cpp)
  accepts a classpath, command directory and main class; non-zygote class startup
  is an explicit branch. The class would use only public LocalSocket/Os APIs.
  No zygote/system-server mode, systemContext, hidden API, reflection, run-as,
  debugger, root, new native binary or permission transition is allowed.
* The host would launch its exact private ADB/serial with non-PTY `shell -T`,
  stdin enabled (never `-n`). The app_process classpath is a pinned owned lab
  artifact path. Only that path, fixed class name, public socket rendezvous name
  and fresh expected UID may appear in argv; never text, editor refs, request
  tokens or encoded payload. No payload in environment variables. Binary stdin
  and stdout must be private pipes managed in memory, not generic command logs.

Client presence/launch, package-path readability, SELinux connectivity and actual
peer identities remain **unverified prerequisites**, not promises from source.
[AOSP shell policy](https://android.googlesource.com/platform/system/sepolicy/+/refs/tags/android-12.0.0_r1/private/shell.te)
and [app policy](https://android.googlesource.com/platform/system/sepolicy/+/refs/tags/android-12.0.0_r1/public/app.te)
are source context, not proof of an installed image's effective policy. A denied
connect, unknown credentials, or identity mismatch stops the experiment. No
filesystem/TCP/forwarding/debug/SELinux fallback or second route is selected.

## Identity and threat model

This is a disposable, exclusively operated lab with pinned APK/JAR hashes and
signing identity, one owned ADB server/AVD and no adversarial shell process. Before
start, resolve the helper package's exact installed base artifact, signing
identity, package UID and Android user using the trusted package-manager route;
bind all of them to this run. No uninstall/reinstall, user switch or package
replacement during the operation. Unknown or unavailable identity stops it.

The server checks kernel SO_PEERCRED via LocalSocket.getPeerCredentials: the
client must be the unprivileged Android shell UID (AOSP shell identity2000; root
or an app UID is refused). The client checks the server UID against that freshly
resolved, pinned helper identity before forwarding any payload. The UID itself
is not a new permission or a configurable exception to admit another caller.

**UID is not invocation authentication.** Every process sharing shell UID can
impersonate the client; processes sharing the helper UID are not isolated by this
check either. A public socket name, PID, or fresh package UID does not fix this.
The random rendezvous name is public, not a secret capability. It may appear as
non-sensitive launch metadata; it is never reused as a request token or editor
ref. Endpoint authenticity is limited to the lab's pinned-package/sole-operator
assumption plus peer UID checks. This is inadequate as a production trust model.

Bind one fresh public abstract name, accept at most one connection, close the
listener immediately after accepting, then check credentials before any request
or reference. A wrong/unknown first peer aborts the whole instance; no accept
loop skips peers until an allowed one arrives. Name collision aborts without
unlinking or replacing anything. LocalServerSocket's OS backlog is not claimed
to be one: its AOSP constructor uses50, while application acceptance is one.

## Proposed protocol and state machine

One instrumentation instance, one connection, at most one issued ref and one
engine entry. All integers are bounded big-endian values; exact magic/version,
closed opcodes and strict UTF8 round-trip are required. No JSON/free-form errors.
Outer frames are length-prefixed, at most **16384 bytes total per frame**, with
at most six logical control/request frames and **32768 bytes total input** per
connection. Before consuming a ref, fully validate that COMMIT frame: unknown
fields/opcodes, duplicate fields, intra-frame trailing bytes, invalid lengths or
truncation invalidate the instance without engine entry. This does not prove
that no further frame will arrive later. Do not wait for EOF to complete a valid
request: EOF has cancellation semantics. A duplicate/invalid late frame or other
protocol violation received after COMMIT invalidates the consumed operation;
before engine entry it prevents that entry, while after possible entry it makes
the outcome sticky UNCERTAIN. No absence-of-future-input guarantee is claimed.

Proposed closed terminal status is REFUSED / UNCERTAIN / VERIFIED. Closed reason
codes are NONE, PEER_UNKNOWN, PEER_MISMATCH, POLICY_DENIED, IDENTITY_CHANGED,
INVALID_FRAME, LIMIT_EXCEEDED, EDITOR_UNSUPPORTED, REF_UNKNOWN, REF_CONSUMED,
REF_EXPIRED, CANCELLED, DEADLINE, ENGINE_REFUSED, ENGINE_UNCERTAIN, TRANSPORT_LOST,
or CLEANUP_UNVERIFIED. Codes never contain exception text, field values or errno
strings. UNKNOWN effect after possible dispatch maps to UNCERTAIN; a refusal reason
cannot downgrade it. Public counters/categories must not include payload hashes.

| State | Accepted input / output | Invariant |
| --- | --- | --- |
| WAIT_PEER | kernel accept / peer check | one peer; no UI ref or payload read before identity |
| OBSERVE | fixed OBSERVE opcode → private REF reply | capture current focused editor; issue one ref in memory |
| ISSUED | COMMIT(ref, appendUTF8) or CANCEL | same runtime/connection, monotone deadline and exact binding |
| CONSUMED | atomically consume ref after complete valid COMMIT, before cache write/engine entry | no second commit; later errors never restore the ref |
| ENGINE_ENTERED | one existing wrapper → TextEngine call | external loss now potentially uncertain; never retry |
| CLEANING | stop/join watcher, close owned FDs, remove only owned scratch | no new operation; preserve any sticky uncertainty |
| TERMINAL | one authoritative fixed wrapper Bundle | registry invalid, socket closed; no further operation or replay |

REF contains only an opaque random run/ref value and bounded lifetime metadata
over the private binary channel, not UI text. The registry retains exact observed
package, resource ID, class, window, bounds, original text and selection. It must
refuse null text, hints/passwords/sensitive types, missing IDs, partial/ambiguous
focus trees or unsupported SET_TEXT capability. The native source is the same
UiAutomation connection used by the wrapper, not a host-supplied snapshot.
Recycle nodes after capture; keep immutable metadata, never a stale node handle.

At COMMIT, the caller supplies only the issued ref and append text, not replacement
identity/bounds/original text. Enforce the engine's256 UTF16-unit before/after
limits and2048-byte individual UTF8-field bound before creating a request.
The server builds after=observedBefore+append and emits the existing binary
request in its own private cache; the engine still freshly checks all identities,
focus, selection, action capability, liveness and readback before/after dispatch.
No clipboard, keyboard remapping, selection repair or alternate backend.

Reference expiry uses **device SystemClock.uptimeMillis**, measured when the ref
is issued, with at most4000ms through dispatch. The host does not supply or extend
absolute expiry. The cache request serializes an immutable expiry no later than
min(originalRefExpiry, instanceDeadline); COMMIT must never replace it with
now+4000ms. Recheck expiry after frame validation and before entry without
recalculating it. A separate bounded handshake budget precedes issuance. One
proposed instance budget is15s, with accept≤5s, per-frame reads≤2s and the existing
engine request≤4s, each capped again by the remaining absolute deadline. Exact
budgets require implementation review; no reset-on-byte timeout or host/device
wall-clock conversion is permitted. A restarted instance generates a new epoch
and rejects old refs; no replay/transport retry after possible engine entry.

## Cancellation, uncertainty and bounded I/O

EOF/CANCEL before COMMIT invalidates the ref and reports cancellation without
engine entry. After consumption but before entry, cancellation prevents the call
and cleans only its scratch request. After entry, a bounded cancellation watcher
may create the engine's existing private cancel marker, but cannot make that
check atomic with performAction. Observed late invalid input, CANCEL, EOF,
exception, marker/write/watcher failure or a lost result after possible entry is
**sticky UNCERTAIN**, never assumed refused or repaired. A later engine verified
or refused callback cannot clear that transport uncertainty. After terminal the
socket is closed: extra traffic cannot start another operation, and no promise
of absence of an earlier effect is made.
No rollback, automatic clear, new instance replay or second mutation follows.
The final fixed engine result can state refused/uncertain/verified; transport
success or an HTTP-like acknowledgement alone is not verified text insertion.

Accept must not be an unbounded worker hidden behind join(timeout). Proposed
public mechanism on API31: get the server FD, set nonblocking mode with
[Os.fcntlInt](https://developer.android.com/reference/android/system/Os#fcntlInt(java.io.FileDescriptor,int,int))
and use [Os.poll](https://developer.android.com/reference/android/system/Os#poll(android.system.StructPollfd[],int))
under the absolute monotone budget. EAGAIN after readiness remains a bounded
readiness race, not an action retry. Accepted-client reads/writes likewise use
nonblocking FD polling; do not assume LocalSocket.connect(timeout) is honored
(AOSP LocalSocketImpl explicitly marks that timeout ignored). At teardown close
owned socket FDs, stop/join every watcher by deadline and prove no thread/FD is
left. Unknown cleanup fails the proof rather than letting a worker run later.

**Build blocker:** fcntlInt is public from API30; the current lab compiler uses
Platform28. The design needs a separately reviewed official Platform31 build
dependency, verified official archive provenance and the applicable existing
stable-license check before any download/install, or a different reviewed public
bounded-I/O design. No hidden/reflection call or unbounded accept workaround is selected.
No SDK package was downloaded or installed for this document.

## Wrapper integration and files proposed after review

The lab entry would inherit NativeTextInstrumentation.onStart, not override it
with a second direct engine runner. A small protected ingress hook in that common
wrapper would obtain a private PendingRequest after connectForText; onStart would
retain the one runOnce call, exception policy, result sink and finish lifecycle.
The lab subclass would override only ingress, never lifecycle or engine dispatch.
The existing standalone missing-token refusal stays a separate regression.
This hook is a proposed wrapper change requiring review, not implemented code.

The common wrapper creates/registers the PendingRequest resource owner before
connectForText or ingress can create an FD, thread or scratch file, so an ingress exception before
return cannot strand unowned resources. Ingress owns ref issuance/atomic
consumption and registers each resource immediately. Only the common onStart
path marks ENGINE_ENTERED immediately before its one runOnce call. The sink stores
a fixed engine result in PendingRequest; it does not publish a terminal outcome.
One bounded watcher observes cancellation/EOF and may create the existing private
cancel marker; it never calls the engine, admits a peer or restores a consumed ref.

One onStart finally path runs PendingRequest.close exactly once, including when
connect/ingress/cache construction throws before engine entry. It invalidates
refs, stops and joins the watcher within the deadline, closes owned FDs, removes
only owned scratch and records each cleanup failure without hiding the primary
failure. Close must not suppress a live watcher/FD/cache failure. An exception or
unverified cleanup after possible entry leaves UNCERTAIN sticky; no stale verified
sink value can become the published result. The common wrapper then calls finish
exactly once with the final allowlisted Bundle, even if close recorded a failure.
The subclass cannot override onStart, terminal publication, close or finish policy.

The **wrapper's final Bundle is the sole authoritative terminal result**, published
after cleanup checks. Socket replies carry REF/progress only, never a competing
terminal verified result. Server-side socket closure is not evidence of success:
the Java app_process client reports only CHANNEL_CLOSED and exits within its
deadline. It never waits for a Bundle or a nonexistent terminal socket message.
Only the HOST waits for and correlates the Bundle on the separately owned
am instrument process's private stdout channel. The watcher
stop/FD-close cutoff must be explicit and must not mistake self-initiated close
for peer EOF; it cannot prove that future or unread late bytes were absent.
If the host cancelled, lost a required channel/result, or cannot correlate the
final Bundle to its one owned invocation, its own uncertainty remains sticky even
if a later Bundle says verified. It never treats EOF or an acknowledgement as
verified, nor invents a refused outcome for a possibly dispatched request.

| Proposed file | Responsibility |
| --- | --- |
| modern/src/.../NativeTextInstrumentation.java | common wrapper lifecycle + narrow ingress hook only |
| emulator/transport/.../LabSocketInstrumentation.java | lab-only ingress; same wrapper entry path |
| emulator/transport/.../LabFrame.java | strict bounded binary codec and closed statuses |
| emulator/transport/.../IssuedEditor.java | in-memory epoch/ref registry, immutable binding, consume once |
| emulator/transport/.../LabSocketClient.java | shell-UID one-shot public LocalSocket stdin bridge |
| emulator/transport/.../PrivateRequest.java | exclusive canonical app-cache request creation/cleanup |
| emulator/transport_host.py | owned private ADB stdin/stdout supervision, no payload logging |
| emulator/tests/transport_* | host fake-peer/clock/wrapper tests, then separately approved AVD proof |
| emulator lab-only manifest/build variant | self-targeting lab entry and client; no permission/service/provider |

Scratch paths are derived solely from the wrapper's canonical private cache and
an internal random token, never a host path. Require exclusive creation, no
symlink/canonical escape, bounded bytes and owner-private files; no overwrite or
external-cache writes. Consume refs before creating the scratch request; TextEngine
still unlinks it before mutation. Cleanup only the exact owned request/cancel/
directory, never arbitrary cache contents. Java text cannot be promised securely
erased from memory; no dumps/logs, payload hashes or exception messages are saved.

## Negative matrix and experiment gates

Host tests first: actual wrapper lifecycle under fakes, malformed/oversized/slow/
truncated frames, unknown opcode, duplicate fields, extra frame, wrong/unknown peer
UID, stale package/user/UID identity, ambiguous/null/sensitive editor, changed
package/resource/class/window/bounds/text/selection, expired/ref reused/conflicting
commit, cancel at every boundary, EOF before/after entry, lost reply, short writes,
blocked accept/read/write, watcher/FD/cache cleanup failure. No-op paths still
check cancellation/deadline. Assert at most one engine entry and no raw output.
Include delayed second frames after COMMIT, cancel racing final close, engine
verified followed by cleanup failure, ingress throwing before PendingRequest is
ready, result loss after entry, and commit near expiry proving the serialized
deadline never moves beyond the initial ref/instance deadline.

Then, only after independent design/code review and a new root-controlled lab GO:

1. Public API compilation + reviewed client/artifact/manifest inspection.
2. Owned AVD connectivity/credential probe with no editor observation or mutation.
   EPERM/EACCES/SELinux denial or unknown credentials stops, without another route.
3. Wrong-UID test without new grants: the existing owned fixture APK may create
   one test connection under its natural app UID; the server must abort before
   accepting OBSERVE/COMMIT. If SELinux blocks it earlier, report policy-blocked,
   not a proven peer-credential test. Mock coverage remains separate. Client-side
   wrong expected server UID can be tested by a negative client expectation only;
   never weaken the server's fixed shell-UID admission rule.
4. Real public wrapper E2E with the same public seeded append and null/sensitive/
   hint/selection/focus/window/cancel/ref-replay cases, both editors unchanged
   where applicable. Secure-setting snapshots and owned cleanup must match run5.

No production transport/capability/MCP wiring follows automatically. Public API
availability, installed-policy compatibility, client execution, peer identities,
bounded accept/cleanup and the actual inherited wrapper path all remain future
proof obligations. This design never authorizes a physical-TV experiment.
