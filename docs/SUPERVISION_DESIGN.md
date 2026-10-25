# Opt-in supervision: implementation contract

This is a separate host lifecycle feature, not a TV command or a promise that a
sleeping/offline Mac remains reachable. Implementation and local fixture tests are
authorized; installing/loading a service or connecting a real TV is not authorized
by this development task. The ordinary installer must not enable this feature.

## Existing access only

The supervisor manages the existing official tunnel runtime. It never executes a
saved TV request. Before each lifecycle action it checks the saved RuntimeIntent,
configuration bytes, installation/tunnel binding, stable launcher, absolute helper
paths, existing ADB identity and private owner marker. Missing/changed/unsafe state
blocks recovery; no discovery, onboarding, identity creation or migration fallback.
Only the already saved, bound runtime credential is loaded. Environment credential
overrides and interactive acquisition are not used. No secret enters service
arguments, configuration, diagnostics or logs. Existing identity/configuration and
operation-journal files remain untouched.

## Policy and durable state

- A private supervision record holds explicit Enabled/Stopped intent, an arm
  revision, bounded counters and closed status codes. A short transaction lock
  protects publication. A separate long owner lease excludes a second supervisor;
  the pipeline session lock is held only around lifecycle actions.
- A fresh, identity-checked observation of the managed process being absent permits
  a start attempt. A live but degraded runtime is observed without blind restart.
  Permission/identity/configuration failures block. HTTP 429 is not repaired by
  changing routes or credentials.
- Persist an attempt before calling the existing start port. Initial bounds are
  three controller starts and three unclean supervisor incarnations per explicit
  arm. Respawn, reboot, clock movement and successful readiness do not reset them.
  Delays are bounded and cancellable. Exhaustion latches Blocked until explicit
  re-arm. An interrupted/in-flight or uncertain start is not replayed; it blocks.
- A cooperative SIGINT/SIGTERM/SIGHUP cancels current work, verifies owned cleanup
  and exits; it does not silently disable a previously enabled service. It never
  ignores hangup. SIGKILL/power loss cannot promise cleanup. An occupied ADB port
  or an unverified orphan blocks instead of killing/adopting another process.
- Only explicit stop disables the durable intent, before draining/stopping the
  owned runtime. A signal/PID record alone never verifies stop. Preserve the
  operation journal, including Unknown receipts, on every path.
- Stop publishes Stopped without waiting for the long owner lease. A start already
  admitted before that publication may have occurred: drain/observe it before
  reporting stop verified. Resume/re-arm refuses unresolved owners, starts or
  cleanup. Every admission, acknowledgement and cleanup is tied to its arm
  revision; an old stop/ack cannot act on a newer arm. Never hold the transaction
  lock while waiting for the owner/pipeline lock or a subprocess.
- Existing `disconnect` also disables a valid owned supervision intent before
  stopping its runtime. It must refuse unsafe/foreign supervision state before
  mutation. Installer uninstall must refuse payload deletion while an owned OS
  registration is present or its removal is unverified; service uninstall comes
  first. A watchdog must never undo an existing explicit stop/uninstall command.
  This uninstall guard runs before every deletion whenever service state exists,
  including corrupt/symlink state and a missing tunnel-owner marker. An older
  binary unable to evaluate the guard fails closed. A foreign launcher is not
  adopted or controlled.
  The first version also refuses payload replacement while its owned service
  registration exists: uninstall the service before updating/downgrading that
  launcher, then opt in again. This prevents an older payload without `service
  run` from causing an OS restart loop outside the durable policy. Legacy
  installers or manual same-UID replacement are outside that guarantee.

## CLI and platform boundary

Host-only CLI commands are `service plan`, `install`, `run`, `status`, `stop`,
`resume` and `uninstall`. They are not MCP tools. Plan is read-only and emits a
reviewable artifact/activation plan. Install and resume require explicit opt-in;
the tests use temporary roots and fake platform commands only. Stop is durable
and cooperative. Uninstall stops first and removes only matching service-owned
artifacts; it preserves TV/tunnel identities, credentials and journal. Unknown
ownership or unverified cleanup is an error, not uninstall success.

The first platform is a per-user macOS LaunchAgent, without sudo, login hooks,
shell profiles or a new network listener. Other platforms return Unsupported.
The plist uses an installation-bound label and a typed ProgramArguments array
containing the stable launcher, `service run` and the absolute configuration path.
There are no shell fragments or credential environment variables. stdout/stderr
go to `/dev/null`; closed events are recorded in bounded private supervisor state.

Use RunAtLoad and KeepAlive/SuccessfulExit=false, with a nonzero throttle and
bounded ExitTimeOut. After verified cooperative cleanup, an Enabled runner exits
nonzero so launchd can relaunch it; an OS logout/shutdown signal does not disarm
the intent. Stopped/Blocked, missing/corrupt state and permission refusal exit
successfully, preventing a restart loop even when state cannot be updated.
Unexpected process termination may be relaunched by launchd, but durable counters
still bound controller actions. OS invocation throttling is distinct from that
action budget. No PathState/WatchPaths authority: the local launchd.plist manual
explicitly describes filesystem watching as race-prone/lossy. Do not parse
`launchctl print`: its manual explicitly says its output is not an API.

Installation first durably records Prepared/Disabled and creates the exact
artifact exclusively, before bootstrap. A bootstrap invocation therefore cannot
connect a TV: the runner remains disarmed. Acknowledged success publishes
Installed, still Stopped. Only a separate explicit resume arms the state and
kickstarts this known-owned job. Failed/lost bootstrap acknowledgement leaves
Unknown/Disabled; it is not retried, adopted or silently assigned a receipt.
Stop may always disarm valid owned state. OS unload/removal requires an Installed
receipt and unchanged artifact; an uncertain installation remains a reported
cleanup blocker with its artifacts preserved for review. Stop racing an install
or resume wins through the same transaction lock/revision check; a late platform
acknowledgement cannot overwrite Stopped or reset a budget.
An entry in Prepared/Stopped performs no backend operation: merely installing the
job must not stop an existing manually started controller. Resume uses kickstart
without `-k` only after the previous owner/start/cleanup has been resolved.

The trust model
is the existing private, exclusively managed user account; hostile same-UID
replacement of a loaded launchd definition is not an isolation guarantee. No
foreign/pre-existing label is adopted. Loaded/recorded state is not readiness;
status distinguishes owner liveness, fresh local observations and historical data.

## Structure and gates

`application/supervision` owns the pure policy/ports; adapters own private state,
official-runtime integration and the macOS artifact/manager boundary. CLI composes
them with the existing saved connection. Keep modules below the source-size cap.

Review this contract before implementation. Test fake clocks and owned fake
processes: crash/restart limits across runner restarts, launcher exit, stop races,
permission/identity refusal, uncertain start, two owners, missing keys, unchanged
identity/journal and no replay. Validate plist syntax with plutil on temporary
files; never load a real job. Exact-source independent review and normal CI precede
publication/integration. Real activation and post-login/wake qualification require
separate user approval and an idle TV; this lot supplies neither.
