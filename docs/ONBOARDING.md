# Guided setup and direct chat connection

`tv-control setup` asks for a LAN interface and a detected display, previews a
private configuration, then asks for confirmation. It does not start ADB, pair,
connect to the device, modify TV settings or create a ChatGPT connection. Device
access and host integration need the explicit steps below. Detection, configured
capabilities, authorization and observed device state are different evidence.

## Download

Version 0.2.0 is an early release. Choose the archive for the **host running the
controller**, not the TV. Archives contain the binary, licence, generic examples
and this guide. ADB and the optional tunnel client remain separate dependencies.

| Host | Release asset |
| --- | --- |
| macOS Apple Silicon, 11+ | `tv-control-aarch64-apple-darwin.tar.gz` |
| macOS Intel, 11+ | `tv-control-x86_64-apple-darwin.tar.gz` |
| Linux x86-64, static musl | `tv-control-x86_64-linux-musl.tar.gz` |
| Linux ARM64, static musl | `tv-control-aarch64-linux-musl.tar.gz` |

From [the release page](https://github.com/antoine-berger/tv-control/releases/tag/v0.2.0),
download the selected archive **and its matching `.sha256` file**. Verify before
extracting or executing. For example on Apple Silicon:

```sh
curl --fail --location --proto '=https' --tlsv1.2 --remote-name https://github.com/antoine-berger/tv-control/releases/download/v0.2.0/tv-control-aarch64-apple-darwin.tar.gz
curl --fail --location --proto '=https' --tlsv1.2 --remote-name https://github.com/antoine-berger/tv-control/releases/download/v0.2.0/tv-control-aarch64-apple-darwin.tar.gz.sha256
shasum -a 256 -c tv-control-aarch64-apple-darwin.tar.gz.sha256
mkdir tv-control-0.2.0
tar -xzf tv-control-aarch64-apple-darwin.tar.gz -C tv-control-0.2.0
cd tv-control-0.2.0
./tv-control --version
```

On Linux use the chosen Linux filename and `sha256sum -c FILE.sha256`. Never
continue after a checksum failure. Checksums downloaded from the same release
protect integrity relative to that release, not independent publisher identity.
Binaries are unsigned/not notarized; any macOS security approval remains a user
action after reviewing provenance. Linux ARM64 static inspection is not evidence
of router firmware compatibility. No package manager or service installation is
performed. Source build remains available in the README.

## Prepare the dependency and run setup

Install maintained [Android SDK Platform Tools](https://developer.android.com/tools/releases/platform-tools)
using the official package for your host. ADB must be on PATH, or select the
absolute executable with `--adb PATH`. Existing system curl is sufficient for an
ADB-only draft; Jellyfin later requires curl plus separately authorized credentials.
The wizard checks executable presence without running them.

```sh
./tv-control setup
```

Choose the LAN interface shared with the TV, rather than a WAN/VPN interface. The
wizard does not request JSON or an IP in the ordinary discovery path. It asks for
an endpoint only as a guided fallback when no usable advertisement is available.
No device-specific app, media title or volume scale is invented. The generated
`tv-control.local.json` has mode 0600, with media and volume actions unconfigured.
Preview includes the selected target and executable; JSON mode includes the full
configuration. Setup requires explicit confirmation before writing.

Saving uses an exclusive temporary file plus atomic no-clobber publication. An
existing destination, including a symlink, is refused. Repeat with `--resume` to
reuse an identical private configuration. If discovery changes a dynamic port or
another value, choose a new output path, review it, then retire the old file
explicitly. No automatic replacement or backup containing private data is made.
EOF, `q` or declining confirmation leaves configuration untouched. An OS interrupt
before publication can leave a private `.tv-control-*.tmp` file; it is never
considered active configuration and may be removed after inspecting ownership.

## What discovery proves

Discovery uses IPv4 only: one mDNS query for ADB services and one SSDP M-SEARCH for
DIAL, from the chosen local interface. mDNS uses its link-local destination
with IP TTL 255; SSDP uses multicast TTL 1. There is no IP/port range
scan, external DNS resolution, HTTP device-description request or ADB subprocess.
Replies are restricted to the chosen subnet and capped at 256 packets, 8192 bytes
per packet and 64 candidate addresses. Duplicate DIAL replies share one slot.
Default duration is 2 seconds; `--duration-ms` accepts 100–5000. No traffic is sent
by `--dry-run` without a fixture. No default interface is selected silently.

This is a **unicast-reply discovery snapshot**, not a continuously subscribed
multicast monitor. The ephemeral socket does not join mDNS port 5353. Received
SRV/A data is associated by structural DNS labels; a target/address disagreement
or missing A record is unresolved and cannot yield a control endpoint. TTL expiry
is based on monotonic receipt time and capped at 30 seconds. Received goodbye
records remove the matching RDATA; they do not remove a newer port. Native receipt
of ordinary multicast announcements/goodbyes is not claimed by parser fixtures.
Rediscover after restart or debugging changes, and if a preview expires.

| Advertisement | Interpretation |
| --- | --- |
| `_adb._tcp` | Legacy TCP debugging; unencrypted, authorization unverified |
| `_adb-tls-pairing._tcp` | Temporary pairing endpoint; never used as a control endpoint |
| `_adb-tls-connect._tcp` | TLS connection endpoint; dynamic port, authorization unverified |
| DIAL SSDP | Display advertisement only; no ADB or Cast capability inferred |

Names are untrusted labels. A name does not establish TV model, OS, compatibility,
identity or authorization. IPv6, cross-VLAN discovery and universal Fire TV
coverage are not implemented. Multiple addresses on one interface can be selected
explicitly as `NAME@IPv4`. Different addresses are not asserted to be one physical
TV. A pairing-only result needs explicit pairing and rediscovery. DIAL-only results
need guided debugging setup or manual fallback. Multiple eligible endpoints stay
ambiguous. Receiver discovery does not implement Cast playback.

Android TV wireless-debugging availability depends on version/vendor; do not
infer TLS support from Android phone support or an Android 12 TV. On Fire OS,
Developer Options/ADB debugging and approval on the TV may be required. Vega and
other non-Android Fire TV models cannot use this ADB adapter.

## Authorize ADB separately

Setup produces a draft, not an active debugging connection. Read
[ADB Wi-Fi architecture](https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/docs/dev/adb_wifi.md)
and your device's instructions before granting broad debugging access. A TLS
pairing code belongs to its temporary pairing endpoint; the connection endpoint
is different. Never include a pairing code in logs, scripts or this configuration.

The controller requires an exclusively owned compatible loopback ADB daemon.
It never starts one itself and must not use an unrelated/shared daemon. The
configuration's default port is 15037. An administrator can prepare a foreground
daemon using maintained ADB, after reviewing key storage and USB/network access:

```sh
adb -L tcp:localhost:15037 --one-device TVCONTROL_NO_USB server nodaemon
```

Keep that foreground process alive during the test; do not supervise/restart or
upgrade it concurrently. Connection/pairing is a **separate explicit ADB action**
to the selected advertised endpoint. The generated configuration supplies the
address so the normal path does not require manually editing JSON. Do not connect
to a pairing port. No connection command is automatically executed by setup.
Creating or accepting a new host key is persistent access requiring review. An
isolated daemon still uses ADB's configured key storage; a separate OS account or
private home is a separate operational choice, not provided automatically.

```sh
./tv-control doctor tv-control.local.json
# Only after approving read-only access to the already authorized target:
./tv-control doctor tv-control.local.json --probe
```

Offline doctor validates configuration and dependency presence. `--probe` uses the
real controller's bounded observe action and never pairs, reconnects or mutates.
An unavailable device remains unverified; the CLI does not silently repair access.
A successful state read does not qualify playback, volume or user-to-TV latency.
The manager-capable ADB executable retains the documented preflight/replacement
race; exclusive daemon ownership is required. See the repository adapter guide.

## Direct ChatGPT/dot connection plan

```sh
./tv-control connect-chatgpt tv-control.local.json
# For an agent:
./tv-control connect-chatgpt tv-control.local.json --json
```

This prints a plan, not a successful connection. It builds shell-quoted stdio
commands with the installed binary/configuration paths, including spaces and
apostrophes. It never reads a key or launches `tunnel-client`.

The supported route uses the official Secure MCP Tunnel client with
`--mcp-command`, so no HTTP rewrite of this controller is needed. Download and
verify the official client separately, review current `tunnel-client help
quickstart`, choose the intended Platform organization/ChatGPT workspace, then
reuse or create an authorized tunnel. Creation needs Tunnels Read + Manage;
runtime and plugin use need Read + Use. Keep its runtime key outside argv and
logs in the supported credential mechanism. No credentials are collected by setup.

After explicit authorization, the printed commands initialize a stdio profile,
run its doctor and keep it alive. In ChatGPT Plugins, add a custom MCP server,
choose Tunnel, select the authorized tunnel, review authentication and the risk
warning, then create/install the plugin. Select it in the intended dot/chat and
verify `tv_capabilities` is actually exposed before a TV action. Workspace access
and permissions are not assumed. There is no delegated chat in this runtime path.
Public source/release publication does not install a public-directory plugin.
No account, tunnel, plugin or permanent service is created by this CLI.

## Offline and agent acceptance

The bundled fixture contains reserved addresses and synthetic advertisements:

```sh
./tv-control discover --fixture examples/discovery.json --interface lan-fixture --json
./tv-control setup --fixture examples/discovery.json --interface lan-fixture --device screen_192_0_2_10 --dry-run --json
```

The second command requires ADB/curl executable presence; use explicit test
executables in a test environment if those dependencies are absent. No executable
is launched by setup. Fixture output is always marked `simulation: true`.
Agent mode uses clean JSON on stdout and never prompts; require explicit interface
and candidate ID, inspect the preview, then repeat with `--confirm` to save.
`--resume` reuses an identical private file; changed configuration is refused.
`--manual-endpoint IPv4:PORT --manual-kind legacy|tls` is a guided fallback bound
to the explicitly chosen subnet. Unsupported options fail instead of being ignored.
Exit 0 means completed offline operation or preview; 2 means a selection/device
setup step remains or confirmation was declined; 1 is an error/EOF cancellation.
None of those statuses imply authorized control or a connected ChatGPT plugin.

## Protocol and product references

- [Multicast DNS](https://www.rfc-editor.org/rfc/rfc6762) and [DNS-SD](https://www.rfc-editor.org/rfc/rfc6763#section-4.3).
- [DIAL specification](https://www.dial-multiscreen.org/dial/protocol-specification).
- [Android ADB guide](https://developer.android.com/tools/adb).
- [Fire TV ADB setup](https://developer.amazon.com/docs/fire-tv/connecting-adb-to-device.html) and [Fire OS overview](https://developer.amazon.com/docs/fire-tv/fire-os-overview.html).
- [Secure MCP Tunnel](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels) and [custom MCP server](https://developers.openai.com/api/docs/guides/custom-mcp-server).
