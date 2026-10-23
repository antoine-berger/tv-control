# Guided connection

The installer guides the stages below. The published v0.3.0 release and newer
source changes are distinct; `latest` is promoted only after release checks.
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
   echo. The official manager supervises the bridge; the bridge owns ADB and MCP.
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
   configuration file. tv-control does not persist the key.
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

Run the same installer after interruption. Private progress remembers choices,
not authorization claims. Each run revalidates access, MCP and runtime health.
A healthy owned controller is reused without asking for its key again. The
installer supplies a verified stable launcher path, preserving the runtime
binding when payload version directories change. A running process can retain
its earlier payload until restarted; previous payloads are preserved.

If the TV configuration changes, the wizard asks before stopping its exact owned
controller. It confirms that the controller stopped, privately preserves the old
launch intent, and starts a new configuration revision. TV and tunnel access must
be approved again. A rejected change, foreign runtime or unverified stop leaves
the old launch intent untouched. An interrupted update resumes from the complete
persisted intent, without treating saved progress as approval.

The connection directory is adjacent to the private TV configuration. It holds
progress, a bounded 64 KiB event-code log, the private ADB home and official tunnel
state. Progress contains no credentials. The launch intent records approved
configuration and launcher choices, not proof of TV authorization. Ephemeral
verification files expire after 120 seconds and are removed on confirmation or
cancellation. Unsafe ownership, links and changed files fail closed.
Each proof also names the bridge's fresh process generation. A controller restart
invalidates the earlier conversation call, so rerun the wizard and verify a new
challenge rather than returning the old code.

Cancelling after background access approval can leave that controller running
for later completion. The explicit advanced stop command is:

```sh
"$HOME/.local/share/tv-control/tv-control" disconnect \
  "${XDG_CONFIG_HOME:-$HOME/.config}/tv-control/tv-control.local.json"
```

Stopping preserves configuration and TV trust. Removing local keys does not
revoke trust on the TV; revoke the computer on the TV when retiring it. No OS
startup service is installed. A runtime that needs its key after restarting
requires re-entry or an explicitly configured supported secret reference.

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
restricted runtime key privately if requested, repeat plugin selection, then obtain
a fresh conversation verification code. Declining leaves the controller running;
setup does not start a second ADB daemon or silently replace an existing runtime.
