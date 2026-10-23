# Guided connection

The v0.3.0 source implements the journey below. It remains subject to review and
release qualification; `latest` is promoted separately.

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
tunnel. A missing tunnel-client is downloaded through the Platform Tunnels page;
this dependency step is currently manual. Reusing an existing tunnel avoids
requiring an administrator credential. The official administrative CLI can
create tunnels separately with the required permissions and explicit approval.

Readiness is a timestamped observation, not a future availability guarantee. Local
MCP discovery, health, `clientInfo`, a received nonce or pressing Enter cannot
prove conversation exposure. Verification is cooperative with the user selecting
the intended chat: public tunnel metadata has no signed conversation identity.

## Resume and stop

Run the same installer after interruption. Private progress remembers choices,
not authorization claims. Each run revalidates access, MCP and runtime health.
A healthy owned controller is reused without asking for its key again. The
installer supplies a verified stable launcher path, preserving the runtime
binding when payload version directories change. A running process can retain
its earlier payload until restarted; previous payloads are preserved.

The connection directory is adjacent to the private TV configuration. It holds
progress, a bounded 64 KiB event-code log, the private ADB home and official tunnel
state. Progress contains no credentials. The launch intent records approved
configuration and launcher choices, not proof of TV authorization. Ephemeral
verification files expire after 120 seconds and are removed on confirmation or
cancellation. Unsafe ownership, links and changed files fail closed.

Cancelling after background access approval can leave that controller running
for later completion. The explicit advanced stop command is:

```sh
tv-control disconnect /absolute/path/to/tv-control.local.json
```

Stopping preserves configuration and TV trust. Removing local keys does not
revoke trust on the TV; revoke the computer on the TV when retiring it. No OS
startup service is installed. A runtime that needs its key after restarting
requires re-entry or an explicitly configured supported secret reference.

## Requirements and limits

- macOS Apple Silicon or Linux ARM64/x86-64; router execution is unqualified.
- Maintained Android Platform Tools and device debugging support. Global ADB keys
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
