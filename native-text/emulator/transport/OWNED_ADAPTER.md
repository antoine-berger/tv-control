# Owned LAB adapter

This adapter is source plus host-only fake tests. It has not run ADB, Android,
Binder, a VM or a physical TV. It does not expose a serial or arbitrary-command
CLI and never starts, reconnects or substitutes an ADB server.

The runner requires an explicit `--scenario existing-proof|lab-connectivity`.
`existing-proof` retains the legacy three-APK contract. `lab-connectivity`
currently requires the reviewed Darwin owned backend, SDK provenance and only
`lab-helper.apk` plus the closed `lab-build-receipt.json` contract. There is no
file-presence-based scenario selection or fallback. A clean source HEAD must
match the source that the reviewed builder recorded before its SDK execution.
The lab branch never calls `run_cases`, launches a fixture activity, sends an
input keyevent, reads an editor or passes append text.

The receipt binds APK SHA256, a signer certificate digest verified by apksigner
at build time, owner source SHA256, source commit and the full validated SDK
provenance receipt hash, with fixed package and instrumentation component.
Receipts are trusted owned local configuration, not authentication against an
attacker who can modify both artifacts and receipts as the same local user.

After the image/ports/API/ABI `owned_ready` gate, the launcher receives the live
PrivateAdb and unreaped owned emulator objects. It verifies the unique ready
emulator transport, both emulator listener owners and the private ADB guard.
Package-manager reads pin the current Android user, a unique helper UID not
shared with another package, and one installed base APK without splits. The
installed base SHA256 must equal the locally verified signed APK bytes. This
also binds those bytes' signing identity; no remote digest becomes a local pin.
The installed base APK itself provides the client's CLASSPATH.

Only the reviewed owner source is pushed into a freshly created per-run owned
`/data/local/tmp` directory. No JAR, grant, chmod, service, permission change,
new ADB identity or persistent helper is introduced by staging. The APK install
is a separate explicit operation on the disposable AVD, not a TV installation.
Hash checks precede use. Local and installed artifact hashes, user/UID/path,
API/ABI and ownership are checked afresh by every mandatory `guard(deadline)`,
including the Host's pre-COMMIT guard if a later reviewed caller uses append.
There is no compatibility fallback to a guard without a deadline.

The fixed process launches use the existing private ADB binary, explicit owned
server address/port/serial, `shell -T` and binary stdin/stdout/stderr pipes.
They never use `shell -n` or generic output capture for protocol data. Each child
is registered by PipedChild before pipe setup; the launcher retains that registry
through partial failure. Existing non-reaping process-group cleanup remains
responsible for local containment. It never substitutes a remote witness.

The operation budget is one absolute deadline, at most fifteen seconds and also
bounded by the outer runtime deadline. Each socket send/receive and lsof call
uses the remaining budget. A slow byte does not restart it. Setup and bounded
containment have separate finite budgets, but cleanup that outlasts work cannot
publish a qualified operation. No abandoned timeout worker thread is used.

`Host.probe_connectivity()` uses the existing supervision loop and sends exactly
one initial CANCEL frame, with no OBSERVE, COMMIT or append. Qualification requires
READY, no REF, a final refused/CANCELLED/entered-NO/cleanup-VERIFIED Bundle,
CHANNEL_CLOSED and the correlated remote-owner normal exit witness. It returns
CONNECTED, which is no claim of a text effect. Normal stdin remains open until
the owner witness. Caller cancellation, malformed proof or timeout remain sticky
Unknown. Missing remote cleanup stops the scenario; there is no automatic replay.

The runner only reserves initially absent scenario-specific packages in its
fresh exclusive AVD. Cleanup checks actual presence, uninstalls only a reserved
present package for the same Android user, requires Success and verifies absence.
It never uninstalls the fixture package in the connectivity branch. Owner-file
cleanup is limited to its own created path. Full AVD teardown is containment,
not proof of client/Binder/watcher cleanup.

These observations are not atomic against another process sharing Android shell
UID or the host user. Fresh guards narrow changes but do not prevent every
check/action race; exclusive operation and the reviewed owned image remain
explicit assumptions. Android socket/SELinux/peer UID and remote cleanup still
need separately authorized runtime qualification after source review.
