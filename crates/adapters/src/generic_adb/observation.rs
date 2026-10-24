//! Intent-specific native evidence, separate from mutation dispatch.
use super::*;

impl GenericAdb {
    pub(super) fn observe_intent(
        &self,
        alias: &str,
        intent: &Intent,
        ctx: &Context<'_>,
    ) -> PortResult<Observed> {
        if let Intent::Media { session, .. } = intent {
            let listing = self.shell(
                alias,
                &["cmd", "media_session", "list-sessions"],
                &[],
                false,
                ctx,
            )?;
            let sessions = media::sessions(&listing)?;
            let (tag, package) = media::select(&sessions, session)?;
            let raw = self.media_dump(alias, ctx)?;
            let snapshot = media::snapshot(&raw, tag, package)?;
            return Ok(Observed {
                connected: true,
                playback: snapshot.playback,
                playback_evidence: PlaybackEvidence::NativeState,
                ..Observed::default()
            });
        }
        if matches!(intent, Intent::Mute | Intent::SetVolume { .. }) {
            // No app/session or configured volume scale is needed to lower to zero.
            // A valid native zero-based range is required before any mutation.
            let raw = self.shell(
                alias,
                &["cmd", "media_session", "volume", "--stream", "3", "--get"],
                &[],
                false,
                ctx,
            )?;
            let (index, max) = parse_volume(&raw)
                .filter(|(_, max)| *max > 0 && *max <= 1000)
                .ok_or(Failure::before("volume_state_unavailable"))?;
            if let Some(ConfigDevice::Adb {
                volume_max_index: Some(configured),
                ..
            }) = self.config.devices.get(alias)
            {
                if *configured != max {
                    return Err(Failure::before("volume_calibration_changed"));
                }
            }
            if let Intent::SetVolume { index: requested } = intent {
                if *requested > max {
                    return Err(Failure::before("volume_out_of_range"));
                }
            }
            return Ok(Observed {
                connected: true,
                volume_max_index: Some(max),
                volume_index: Some(index),
                ..Observed::default()
            });
        }
        if *intent == Intent::Wake {
            // No foreground activity or media session is required while asleep.
            let raw = self
                .shell_bytes(
                    alias,
                    &["dumpsys", "power"],
                    &[],
                    false,
                    MAX_POWER_DUMP_BYTES,
                    ctx,
                )
                .and_then(|raw| {
                    String::from_utf8(raw).map_err(|_| Failure::before("adb_read_failed"))
                })
                .map_err(|e| {
                    if e.code == "adb_read_failed" {
                        Failure::before("wake_transport_unavailable")
                    } else {
                        e
                    }
                })?;
            return Ok(Observed {
                connected: true,
                awake: parse_awake(&raw),
                ..Observed::default()
            });
        }
        let package = self.foreground(alias, ctx)?;
        let mut observed = Observed {
            connected: true,
            active_app: package.clone(),
            playback_evidence: PlaybackEvidence::NativeState,
            ..Observed::default()
        };
        if matches!(intent, Intent::Pause | Intent::Resume) {
            if let Some(package) = package {
                let raw = self.shell(alias, &["dumpsys", "media_session"], &[], false, ctx)?;
                observed.playback = parse_playback(&raw, &package)?;
            }
        }
        Ok(observed)
    }
    pub(super) fn observe_state(&self, alias: &str, ctx: &Context<'_>) -> PortResult<Observed> {
        // A successful targeted shell observation already proves transport connectivity.
        let package = self.foreground(alias, ctx)?;
        let mut observed = Observed {
            connected: true,
            active_app: package.clone(),
            playback_evidence: PlaybackEvidence::NativeState,
            ..Observed::default()
        };
        if let Some(package) = package {
            let raw = self.shell(alias, &["dumpsys", "media_session"], &[], false, ctx)?;
            observed.playback = parse_playback(&raw, &package)?;
        }
        if let Some(ConfigDevice::Adb {
            volume_max_index: Some(max),
            ..
        }) = self.config.devices.get(alias)
        {
            let raw = self.shell(
                alias,
                &["cmd", "media_session", "volume", "--stream", "3", "--get"],
                &[],
                false,
                ctx,
            )?;
            let (index, actual_max) =
                parse_volume(&raw).ok_or(Failure::before("volume_state_unavailable"))?;
            if actual_max != *max {
                return Err(Failure::before("volume_calibration_changed"));
            }
            observed.volume_index = Some(index);
        }
        Ok(observed)
    }
}

/// Accept only the top-level PowerManager wakefulness field, not per-display or
/// historical log entries. Ambiguous/unknown vendor formats remain unverified.
fn parse_awake(raw: &str) -> Option<bool> {
    let mut values = raw
        .lines()
        .filter_map(|line| line.trim().strip_prefix("mWakefulness="));
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    match value.trim() {
        "Awake" => Some(true),
        "Asleep" | "Dozing" | "Dreaming" => Some(false),
        _ => None,
    }
}
