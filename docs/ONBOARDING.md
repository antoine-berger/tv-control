# Guided connection

The installer guides first installation through the stages below. Published releases
and newer source changes are distinct; `latest` is promoted only after release checks.
Device and hosted-chat acceptance are separate from automated release checks.

```sh
curl -fsSL https://raw.githubusercontent.com/antoine-berger/tv-control/latest/install.sh | sh
```

The terminal guides four stages:

1. **TV access.** Select the TV and approve a private ADB identity and connection.
   Authorize that identity on the TV when prompted. The dedicated daemon stays
   alive while the user approves; the shared ADB daemon is never adopted.
2. **Local controller.** Observe the TV without changing playback and negotiate
   MCP sequentially. Connection acknowledgements and dispatch alone do not pass.
3. **ChatGPT connection.** Use the official tunnel-client and an existing tunnel.
   Approve background TV access, then enter its restricted runtime key without
   echo, then choose whether to save it privately for future updates. The official manager supervises the bridge; the bridge owns ADB and MCP.
   The controller continues after the installer finishes.
4. **Conversation verification.** Create/install the custom tunnel plugin in
   ChatGPT and select it in the intended conversation. The wizard supplies a
   fresh challenge for `tv_verify_connection`. That tool makes a harmless Observe
   call and returns a code. Copy the code back to the terminal. The wizard checks
   the binding, rechecks the runtime and observes the TV again before reporting ready.

Account/workspace permissions can prevent connection. The wizard guides those
steps; it does not provision an administrator key or silently create a remote
tunnel. When tunnel-client is missing, the same wizard offers an explicit dependency
installation: a pinned private official release on Linux, or the supported official
Homebrew formula on macOS. See [dependency acquisition](INSTALLATION.md#connection-helper).
Reusing an existing tunnel avoids
requiring an administrator credential. The official administrative CLI can
create tunnels separately with the required permissions and explicit approval.

Readiness is a timestamped observation, not a future availability guarantee. Local
MCP discovery, health, `clientInfo`, a received nonce or pressing Enter cannot
prove conversation exposure. Verification is cooperative with the user selecting
the intended chat: public tunnel metadata has no signed conversation identity.

## Browser steps for the OpenAI connection

The wizard prints each link before it asks for the corresponding value:

1. Open [Platform Tunnels](https://platform.openai.com/settings/organization/tunnels)
   and select the intended organization. Personal accounts use their personal
   Platform organization. Open or create a tunnel and include the target ChatGPT
   workspace in its associations. Copy the complete `tunnel_...` identifier into
   **Tunnel ID**. Creating a tunnel requires Tunnels **Read + Manage**.
2. After reviewing the background-controller consent, open
   [Runtime API keys](https://platform.openai.com/settings/organization/api-keys)
   in the same organization. Create a **Restricted** key with Tunnels **Read + Use**;
   its owner also needs those organization permissions. Do not choose **All** or
   use an administrator key. Enter the secret yourself in the terminal's hidden
   **Runtime key** prompt, never in chat, screenshots, command arguments or a
   TV configuration file. Saving the key is a separate, explicit choice; see
   [credential storage and recovery](#credential-storage-and-recovery).
3. Once the controller is ready, open [ChatGPT Plugins](https://chatgpt.com/plugins)
   in a browser. Select **+ → Add custom MCP server**, give it a name, and choose
   **Tunnel** under Connection. Select or paste the same tunnel ID. This project's
   local stdio server uses **No authentication** at the MCP layer; OpenAI tunnel
   organization/workspace permissions still apply. Review the risk warning
   yourself, create the plugin, then install it. In the intended conversation,
   type `@` and select that plugin before completing the verification prompt.

If Platform access is blocked, type `help` at **Tunnel ID** for the organization
roles/groups links. An organization owner or RBAC administrator must grant the
required permissions; changes can take up to 30 minutes. ChatGPT custom MCP access
is separately subject to workspace permissions and security restrictions. The
installer cannot grant those permissions or guarantee account eligibility. No
subscription, billing change or extra purchase is performed by this wizard.
Use `q` to stop and resume after resolving account access. A newly created tunnel
may take 25–30 seconds to become available; keep the controller running during
plugin discovery and use.

The exact signed-in Platform button labels may change. The destinations, key
restriction and permission names above come from the [official onboarding guide](https://github.com/openai/tunnel-client/blob/master/docs/onboarding.md),
[permissions guide](https://github.com/openai/tunnel-client/blob/master/docs/permissions.md)
and [custom MCP instructions](https://developers.openai.com/api/docs/guides/custom-mcp-server).

## TV authorization recovery

An ADB `unauthorized` state means the private controller is not trusted; it does
not prove that the TV displayed a dialog. Press Enter to check again, or type
`retry` to disconnect and reconnect only the configured TV with the same private
identity and owned daemon. This cannot guarantee a new TV dialog. Missing/offline
transports and handshakes in progress have distinct guidance, never an approval
claim. Each connection attempt has a 12-second local bound within the journey's
overall deadline; TV command and discovery budgets are unchanged. `q` or Ctrl-C
stops this foreground attempt without deleting identity files or resetting TV trust.

If no authorization dialog appears while ADB reports `unauthorized`:

1. Enter `q` to stop the foreground attempt.
2. Use the TV's full **Restart** action, rather than standby. After it has fully
   restarted, run the same installer again and approve the dialog if it appears.
3. If the dialog is still absent, check the TV's debugging settings and vendor
   documentation. Do not delete the controller identity or revoke all trusted
   computers as an automatic retry step.

A restart is troubleshooting, not a guaranteed fix. [AOSP's authorization service](https://android.googlesource.com/platform/frameworks/native/+/refs/heads/main/libs/adbd_auth/adbd_auth.cpp)
queues prompts and tracks the dispatched prompt separately from authorized
connections. A stale pending prompt is therefore a possible explanation for a
missing dialog, not a confirmed diagnosis of this issue.

## Resume and stop

Run the same installer to update an existing installation or resume an interrupted
one. A valid saved configuration skips TV selection and pairing. On an exact
owned connection, maintenance preserves the device identity, tunnel ID and approved
access instead of replaying first-install questions. Additional device discovery
is optional and list-only: it does not pair, switch devices or replace configuration.

A healthy owned controller can remain running without entering a key. If no saved
key is available, configuring one is optional; declining leaves that process alone.
To activate the installed payload, accept the explicit restart prompt. The controller
resolves a saved or newly supplied key **before** stopping the old process. A saved
key is reused without asking you to enter it again. A stopped or unhealthy saved
controller requires an explicit resume confirmation, not a fresh installation.
An unavailable or malformed response from the owned runtime's optional local MCP
health endpoint is recorded as not ready. It does not prevent that explicit
recovery prompt. Invalid endpoint URLs, runtime identity/ownership failures,
permission errors, cancellation and expiry still stop maintenance. No failed
health request is retried automatically, and startup must pass fresh readiness
checks after an approved restart.
The verified stable launcher preserves ownership across payload-version changes;
old payloads remain available, and a running process retains its earlier code until
restarted. Installation alone does not mean that new code is running.

Changed configuration or expanded access, such as first enabling generic ADB
control, requires separate approval. The required key is prepared before the old
controller can be stopped. Declining migration preserves the existing controller.
A foreign runtime, unsafe saved state or unverified stop fails closed rather than
silently adopting it or starting a second controller.

Maintenance reports local readiness only. It does not claim a new successful call
from your conversation. A restart invalidates old conversation proof; explicit
onboarding verification issues a fresh challenge when that evidence is needed.

If a read-only local preflight returns `adb_server_unavailable`, the configured
private ADB listener was unavailable. A mutation that may already have been
dispatched remains uncertain: recover its operation instead of replaying it.
Historical progress and build records do not establish that the controller is
still running. Use the
existing installation's maintenance route when reconnection is authorized; do not
start a global ADB server, delete the journal or replace credentials as a shortcut.
An HTTP 429 from the hosted plugin is separate evidence: a local recovery test
does not identify or repair an external tunnel provider's rate limit.

The connection directory is adjacent to the private TV configuration. It holds
progress, a bounded 64 KiB event-code log, the private ADB home and official tunnel
state. Progress contains no credentials. The launch intent records approved
configuration and launcher choices, not proof of TV authorization. Ephemeral
verification files expire after 120 seconds and are removed on confirmation or
cancellation. Unsafe ownership, links and changed files fail closed.

### Credential storage and recovery

Releases through v0.4.0 deliberately kept the runtime key only in memory and passed
it to the official helper through its environment. They did **not** save it in the
TV configuration. A running process is not evidence of durable storage, and a
stopped process provides no supported way to recover that old key. If it was never
saved elsewhere and is now unavailable, create a replacement restricted runtime
key for the same tunnel and enter it at the hidden terminal prompt. Never send it
in chat or use an administrator key.

New credential persistence is opt-in: the terminal asks before writing
`connection/runtime-credential.json`. This is **unencrypted plaintext**, separate
from the TV configuration and logs, restricted to your user with file mode `0600`
in the private `0700` connection directory. It is not an OS keychain and does not
protect against software already running as your user. The record is bound to
this installation and tunnel; it is not reused for another connection. Declining
storage keeps the supplied key in memory only, so a future restart needs it again.

For advanced use, an explicitly supplied `TV_CONTROL_TUNNEL_KEY` overrides a valid
saved key and offers the same save choice. Supply it through an appropriate secret
manager; do not put the value in command arguments, shell history, logs or examples.
An override does not bypass checks on corrupt, foreign or insecure stored state.

Missing storage triggers recovery guidance; unreadable or invalid storage is an
error, never silently treated as missing. Do not loosen permissions or delete state
blindly to bypass it. Check the error code and restore a known-good private backup
bound to this installation, or obtain help inspecting metadata without displaying
the key. Unexpected hard links, including a write interrupted during atomic
publication, need explicit review before repair. Keep a still-running controller
running while resolving key availability. Rotate a key if its privacy is in doubt.

Cancelling after background access approval can leave that controller running
for later completion. The explicit advanced stop command is:

```sh
"$HOME/.local/share/tv-control/tv-control" disconnect \
  "${XDG_CONFIG_HOME:-$HOME/.config}/tv-control/tv-control.local.json"
```

Stopping preserves configuration and TV trust. Removing local keys does not
revoke trust on the TV; revoke the computer on the TV when retiring it. No OS
startup service is installed. A future restart reuses an opted-in saved runtime key;
without one it needs a separately supplied key.

## Requirements and limits

- macOS Apple Silicon or Linux ARM64/x86-64; router execution is unqualified.
- Maintained Android Platform Tools (guided acquisition is available on supported hosts) and device debugging support. Global ADB keys
  are not read or copied; the new private identity needs its own TV authorization.
- Legacy TCP ADB is unencrypted on the LAN. The Internet tunnel does not encrypt
  that leg. Pairing ports are not control ports. Automatic new TLS ADB pairing is
  not implemented by this journey.
- Runtime access requires Tunnels Read + Use; creation/editing requires Read +
  Manage. The intended organization and ChatGPT workspace must be eligible.
- The current TV setup creates an ADB configuration. Media app selection, volume
  calibration and Jellyfin credentials remain explicit configuration work.
- Fixtures test process/stdio/loopback composition. They do not establish real
  TV authorization, ChatGPT availability or the ten-second chat-to-TV target.

## Advanced checks

`setup` prepares configuration; the installer follows it with `onboard`.
`onboard CONFIG --json` returns pending without access grants. The explicit
`onboard CONFIG --fixture FIXTURE --json` path composes only simulated ports and
always returns `ready:false`. `doctor CONFIG` is offline; `doctor CONFIG --probe`
is an explicitly requested read-only device call. `connect-chatgpt CONFIG` prints
the manual connection plan.

Sources: [Secure MCP Tunnels](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels),
[ChatGPT plugin connection](https://developers.openai.com/plugins/deploy/connect-chatgpt),
[official runtime lifecycle](https://github.com/openai/tunnel-client/blob/master/plugins/tunnel-mcp/skills/tunnel-mcp/references/runtime-flows.md),
[local health semantics](https://github.com/openai/tunnel-client/blob/master/docs/health.md).

### MCP setup fails while the tunnel is running

A running tunnel and a successful `/readyz` response do not establish MCP discovery.
Setup checks the same runtime's passive `/health/mcp` result after you return from
plugin selection. `tunnel_mcp_discovery_failed` means that initialization or tool
discovery failed; it does not mean that your TV trust was lost.

The server supports repeated, validated legacy MCP handshakes on the official
client's shared stdio process. Each handshake still requires a fresh initialized
notification. Controller ownership, request deduplication and pending action
cancellation survive that handshake.

If an older controller is still running with failed discovery, rerun the installer.
Accept the explicit prompt to restart this owned controller with the installed
version. This keeps its TV identity, configuration and tunnel ID. Enter the
restricted runtime key privately only if none is available. Maintenance checks
local readiness; use explicit conversation verification when a fresh chat proof is needed. Declining leaves the controller running;
setup does not start a second ADB daemon or silently replace an existing runtime.

### Resume or repeat conversation verification explicitly

Maintenance does not repeat the plugin UI/challenge wizard after completed setup.
If first setup was cancelled at conversation verification, the normal installer
offers to resume just that step; no extra flags are required. Declining leaves
it pending. To deliberately request a fresh proof at any other time, use the
same configuration and installed stable launcher:

```sh
"$LAUNCHER" verify-connection "$CONFIG" --launcher "$LAUNCHER"
```

Here `LAUNCHER` is the absolute installed `tv-control` launcher path and `CONFIG`
is the existing private TV configuration path. For a direct, non-installer
binary installation, omit `--launcher` only if that exact binary owns the saved
runtime intent. This command requires the existing controller to be locally
ready and checks its exact identity and generation. It does not start or stop a
controller, request a key, rediscover devices, or grant additional access. It
issues a new challenge and only marks verification complete after the selected
conversation makes the actual read-only call and its returned code is checked.
