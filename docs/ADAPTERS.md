# Adapter requirements and limits

Adapters use installation details from [configuration](CONFIGURATION.md), not
hosts, URLs or shell strings supplied in a request. They invoke configured ADB
and curl executables; neither executable, an ADB daemon nor a tunnel is bundled.

| Backend | Implemented | Unavailable |
| --- | --- | --- |
| ADB | State reads, TV app launch, targeted media pause/play, calibrated or generic native-range music stream volume, generic music-stream mute | Discovery, pairing, connection setup, media catalogue or title selection |
| Jellyfin HTTP via curl | Exact session selection, pause/unpause, configured series resume at saved position, postcondition reads, bounded catalog/episode/source/track metadata, explicit item/source playback, absolute/relative source-bound seek and client-capability-bound audio/subtitle selection | App launch, volume, library mutation or credential creation |
| YouTube / YouTube Music | Generic ADB app launch and native media controls where supported | Search, lyrics, song/video mode, account navigation or a service-specific Cast adapter |
| Cast / Android TV Remote v2 / UIAutomator | No implemented backend | These are extension possibilities, not working adapters |

## ADB

The explicit target must already be connected to an authorized, compatible daemon.
The adapter reads foreground activity, native media state and optional volume
using Android command output. Vendor output must match its parsers; support is
not universal across Android TV versions and apps.

App launch resolves the package's `LEANBACK_LAUNCHER` activity and checks the
foreground app afterward. Pause/resume requires a configured `media_app` and one
unambiguous media-session tag. Tags duplicated across any package are rejected
because Android's monitor command selects by tag alone.

Legacy `set_volume` is disabled unless `volume_max_index` is configured and matches
the observed maximum. With the explicit generic ADB grant, `set_volume` probes
the native range instead and does not require calibration; a configured maximum
must still match. `tv_read` with `read: {"type":"volume"}` exposes both
`volume_index` and `volume_max_index` from that device. The unit is the Android
music stream index. It is neither
TV percent nor an external receiver's level, and need not match the number shown
by a television.

Generic relative and percentage volume use the same native music-stream evidence,
with a freshly resolved absolute target and a range recheck before mutation. They
require the existing generic grant and do not identify the physical output or
implement restoration. See [relative volume semantics](MCP.md#relative-and-percentage-volume).

Generic `mute` is separately available under the explicit generic ADB grant. It
requires readable Android music stream 3 volume, sets that stream to zero, and
verifies zero afterward. An already-zero stream needs no write. It does not
require `volume_max_index`, send a mute-toggle key, increase volume, or implement
unmute/restore. Device commands and audio routing vary: a zero music-stream
index does not prove external HDMI/CEC speakers are silent. Unsupported or
unreadable state must not become a verified result. See [MCP mute semantics](MCP.md#idempotent-music-stream-mute).

### ADB daemon ownership

**Use an isolated daemon exclusively owned by the controller.** Keep its smart
socket on IPv4 loopback and do not share it with unrelated clients or replace it
concurrently. A dedicated unprivileged account is preferable where practical.
This repository does not create or install that daemon.

The ADB executable can manage its server: AOSP clients may kill and restart an
incompatible daemon even for a device-read command. Before each connected
invocation, the adapter compares local `adb version` output with a read-only
smart-socket `host:version` probe. Known mismatches are rejected before a device
command. The probe itself never sends `host:kill`; a fake-server regression
checks the incompatible-server case.

A daemon can still be replaced between probe and invocation. This check/use race
is a remaining limitation, so the guard does not establish an unconditional
no-restart guarantee. A transport that cannot manage the daemon would be needed
for that stronger contract.

ADB authorization grants broader device access than this tool exposes. Legacy
TCP debugging is not encrypted by RSA authorization; an Internet tunnel does not
encrypt that separate LAN leg. An old router ADB package may lack modern
pairing/TLS. See [Android's wireless ADB design](https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/docs/dev/adb_wifi.md).

## Jellyfin

The adapter checks `/System/Info/Public`, then selects exactly one remotely
controllable session matching the configured user and device IDs. The current
version gate accepts numeric server versions with major version 12 or later and
rejects prereleases. Version 12.0 is the patched baseline for
[GHSA-4vx8-xhc9-qg6x](https://github.com/jellyfin/jellyfin/security/advisories/GHSA-4vx8-xhc9-qg6x).
This is a compatibility/security baseline, not a complete vulnerability assessment
or proof that every later server/client combination works.

Configured series resume reads `/UserItems/Resume` for the exact user and series,
requires a matching series and nonzero saved position, and dispatches
`/Sessions/{id}/Playing` with `playCommand=PlayNow`, `itemIds` and
`startPositionTicks`. A `204` response acknowledges dispatch only. A compatible,
already authenticated client session must exist.

Some Android TV clients handle Pause/Unpause as toggles. The adapter checks current
state immediately before sending once and avoids a redundant command. A physical
remote can still change state between that read and dispatch; there is no
compare-and-set operation. Ambiguous commands are not automatically retried.

Audio/subtitle selection uses the API's JSON `GeneralCommand` form:
`POST /Sessions/{id}/Command`, with a closed command `Name` and string `Index`
inside `Arguments`. The `/Command/{command}` shortcut does not convey the index.
The bounded JSON body is escaped into curl's stdin configuration alongside the
existing token; neither is put in process arguments or returned in diagnostics.
Session `SupportedCommands` and media-control support must be fresh, in addition
to exact item/source/track metadata. Null selected indices are unknown. See the
[source-bound contract](MCP.md#relative-seeking-and-source-bound-track-selection).

### Credential handling

Only the HTTP adapter reads the configured token environment variable. It passes
the value to curl through stdin, not command arguments. Raw subprocess stderr is
not returned as tool output. curl configuration inheritance, redirects, URL
globbing and proxy use are disabled; TLS certificate validation remains enabled.
Cleartext HTTP requires explicit configuration. No secure erasure of process
memory is claimed.

Executables are trusted administrator configuration. Bounded pipes and process
group cleanup limit local work; they are not a sandbox for hostile executables.

## Playback evidence

Native Android `PlaybackState` and Jellyfin's pause flag provide different
evidence. A native playing state can verify `resume`; Jellyfin requires matching
media identity and increasing positions, not merely `IsPaused=false`. Missing or
null Jellyfin positions cannot verify resume.

`resume_media` requires the requested item and two observations with increasing
position. Positions must be within `resume_position_tolerance_ms` of the requested
start, enlarged only by elapsed time since dispatch; far-ahead progress is an
unknown outcome. A foreground app verifies app launch only.

These are client-reported postconditions. They do not prove a physical first
video frame, audible output or a ten-second user-to-TV result. The
[acceptance protocol](ACCEPTANCE.md) separates those measurements from controller
and fixture tests. Keep real configuration, keys, state dumps, captures and
viewing history out of public issues and Git.
