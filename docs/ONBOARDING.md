# Onboarding implementation status

The onboarding work is delivered in stages. This first stage adds discovery
ports, domain policy, a native UDP adapter and offline wire fixtures. It does
**not yet expose a discovery/setup CLI command**. The existing control commands
are unchanged. No real network discovery or device authorization was performed
during development.

## Scope and boundaries

The planned CLI flow is interface choice, bounded discovery, device choice,
configuration preview and explicit confirmation. An agent will use the same
operations with structured JSON. Configuration generation must not imply that
ADB access or device capabilities have been verified. Pairing, debugging changes,
daemon ownership and ChatGPT permissions are separate steps.

Discovery currently implements IPv4 mDNS for the three ADB service types and
SSDP M-SEARCH for DIAL. The adapter sends one query per protocol, with multicast
TTL 1, from the selected local interface address; it never scans address/port
ranges or follows SSDP LOCATION URLs. There is no ADB subprocess in discovery,
so discovery cannot start an ADB daemon or trigger its auto-connect behavior.
Replies are limited to the selected subnet, 256 datagrams, 8192 bytes per packet
and a caller-supplied cancellable deadline. Results are bounded and deduplicated
by advertiser address, not by an asserted physical-device identity. IPv6 and
cross-VLAN discovery are not implemented. Multi-address interface names require
unambiguous selection and currently return a diagnostic instead of guessing.

mDNS service identities are preserved, goodbye records remove withdrawn services,
and newer advertisements replace dynamic ports. Advertised ports remain
unverified until a later connection diagnostic. They must be rediscovered after
wireless debugging is toggled or a device restarts.

| Advertisement | Interpretation |
| --- | --- |
| `_adb._tcp` | Legacy TCP debugging; unencrypted, authorization unverified |
| `_adb-tls-pairing._tcp` | Temporary pairing endpoint; never used as a control endpoint |
| `_adb-tls-connect._tcp` | TLS connection endpoint with a dynamic port; authorization unverified |
| DIAL SSDP | Display advertisement only; no ADB or Cast control capability inferred |

A DIAL-only result needs guided debugging setup or a manual endpoint fallback.
A pairing-only result needs explicit pairing on the device and rediscovery.
Multiple eligible endpoints remain ambiguous. Android TV version and vendor
support cannot be inferred from a name. Fire TV devices do not universally
advertise ADB, and non-Android models cannot use this adapter.

## Connection and distribution work still pending

The next stage adds the interactive/JSON CLI, safe private configuration writes,
preview/confirmation, recovery and dry-run tests. The ChatGPT command will only
prepare the official Secure MCP Tunnel stdio command and manual approval steps;
it will not create credentials, start a tunnel or claim an installed plugin.
Release archives and checksums must actually be published before describing a
binary-download path as available. The existing source-build path remains valid.

## Sources

- [ADB Wi-Fi architecture](https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/docs/dev/adb_wifi.md): service roles, dynamic TLS ports and automatic connections by the ADB daemon.
- [Multicast DNS, RFC 6762](https://www.rfc-editor.org/rfc/rfc6762): local-link discovery and DNS wire behavior.
- [DIAL specification](https://www.dial-multiscreen.org/dial/protocol-specification): display discovery protocol.
- [Secure MCP Tunnel](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels): supported stdio command and tunnel lifecycle.
- [Custom MCP server](https://developers.openai.com/api/docs/guides/custom-mcp-server): plugin creation and workspace approval steps.
