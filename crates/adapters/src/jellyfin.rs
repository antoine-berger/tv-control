mod catalogue;
mod tracks;
use crate::{
    config::{Config, Device},
    http::{encode, Http, HttpRequest},
};
use serde::Deserialize;
use tv_application::Context;
use tv_domain::*;

pub struct Jellyfin<'a> {
    config: &'a Config,
    http: &'a dyn Http,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ServerInfo {
    version: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Session {
    id: String,
    device_id: String,
    user_id: Option<String>,
    supports_remote_control: Option<bool>,
    supports_media_control: Option<bool>,
    client: Option<String>,
    application_version: Option<String>,
    #[serde(default)]
    supported_commands: Vec<String>,
    #[serde(default)]
    playable_media_types: Vec<String>,
    now_playing_item: Option<Item>,
    play_state: Option<PlayState>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Item {
    id: String,
    #[serde(default)]
    series_id: Option<String>,
    #[serde(default)]
    user_data: Option<UserData>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PlayState {
    is_paused: bool,
    position_ticks: Option<u64>,
    can_seek: Option<bool>,
    media_source_id: Option<String>,
    audio_stream_index: Option<i32>,
    subtitle_stream_index: Option<i32>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UserData {
    playback_position_ticks: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Items {
    items: Vec<Item>,
}

impl<'a> Jellyfin<'a> {
    pub fn new(config: &'a Config, http: &'a dyn Http) -> Self {
        Self { config, http }
    }
    fn device(&self, alias: &str) -> PortResult<(&str, &str, &str)> {
        match self.config.devices.get(alias) {
            Some(Device::Jellyfin {
                server,
                user_id,
                device_id,
                ..
            }) => Ok((server, user_id, device_id)),
            _ => Err(Failure::unsupported("unsupported")),
        }
    }
    fn get<T: serde::de::DeserializeOwned>(
        &self,
        server: &str,
        path: String,
        ctx: &Context<'_>,
    ) -> PortResult<T> {
        let response = self.http.send(
            HttpRequest {
                server: server.into(),
                method: "GET",
                path,
                mutating: false,
                json_body: None,
            },
            ctx,
        )?;
        if response.status != 200 {
            return Err(Failure::before("jellyfin_read_failed"));
        }
        serde_json::from_slice(&response.body)
            .map_err(|_| Failure::before("jellyfin_invalid_response"))
    }
    fn check_version(&self, server: &str, ctx: &Context<'_>) -> PortResult<()> {
        let version: ServerInfo = self.get(server, "/System/Info/Public".into(), ctx)?;
        if !patched_version(&version.version) {
            return Err(Failure::before("jellyfin_server_requires_security_review"));
        }
        Ok(())
    }
    fn session(&self, alias: &str, ctx: &Context<'_>) -> PortResult<(String, Session)> {
        let (server, user, device) = self.device(alias)?;
        self.check_version(server, ctx)?;
        let sessions: Vec<Session> = self.get(
            server,
            format!("/Sessions?deviceId={}", encode(device)),
            ctx,
        )?;
        let mut matching = sessions
            .into_iter()
            .filter(|s| s.device_id == device && s.user_id.as_deref() == Some(user));
        let session = matching
            .next()
            .ok_or(Failure::before("jellyfin_session_not_found"))?;
        if matching.next().is_some() || !crate::config::opaque_id(&session.id) {
            return Err(Failure::before("jellyfin_session_ambiguous"));
        }
        if session.supports_remote_control != Some(true) {
            return Err(Failure::before("jellyfin_remote_control_unavailable"));
        }
        Ok((server.into(), session))
    }
    pub fn observe(&self, alias: &str, ctx: &Context<'_>) -> PortResult<Observed> {
        let (_, session) = self.session(alias, ctx)?;
        let playback = match (&session.now_playing_item, &session.play_state) {
            (Some(_), Some(p)) if p.is_paused => Playback::Paused,
            (Some(_), Some(_)) => Playback::Playing,
            (None, _) => Playback::Stopped,
            _ => Playback::Unknown,
        };
        Ok(Observed {
            awake: None,
            connected: true,
            active_app: None,
            playback,
            playback_evidence: PlaybackEvidence::PositionProgressRequired,
            media_id: session.now_playing_item.map(|i| i.id),
            media_source_id: session
                .play_state
                .as_ref()
                .and_then(|p| p.media_source_id.clone()),
            position_ticks: session.play_state.as_ref().and_then(|p| p.position_ticks),
            audio_index: session
                .play_state
                .as_ref()
                .and_then(|p| p.audio_stream_index),
            subtitle_index: session.play_state.and_then(|p| p.subtitle_stream_index),
            volume_index: None,
            volume_max_index: None,
        })
    }
    pub fn resolve(&self, device: &str, alias: &str, ctx: &Context<'_>) -> PortResult<MediaPlan> {
        let (server, user, _) = self.device(device)?;
        let media = self
            .config
            .media
            .get(alias)
            .ok_or(Failure::before("unknown_media_alias"))?;
        if media.server != server {
            return Err(Failure::before("media_server_mismatch"));
        }
        let result: Items = self.get(server, format!("/UserItems/Resume?userId={}&parentId={}&limit=1&enableUserData=true&enableImages=false&enableTotalRecordCount=false", encode(user), encode(&media.series_id)), ctx)?;
        let item = result
            .items
            .into_iter()
            .next()
            .ok_or(Failure::before("no_resumable_media"))?;
        if item.series_id.as_deref() != Some(&media.series_id)
            || !crate::config::opaque_id(&item.id)
        {
            return Err(Failure::before("resume_media_identity_mismatch"));
        }
        let position = item
            .user_data
            .ok_or(Failure::before("resume_position_missing"))?
            .playback_position_ticks;
        if position == 0 {
            return Err(Failure::before("resume_position_missing"));
        }
        Ok(MediaPlan {
            item_id: item.id,
            source_id: None,
            media_kind: None,
            position_ticks: position,
        })
    }
    pub fn apply(
        &self,
        alias: &str,
        action: &Action,
        plan: Option<&MediaPlan>,
        ctx: &Context<'_>,
    ) -> PortResult<()> {
        if matches!(
            action,
            Action::SelectAudioTrack { .. } | Action::SelectSubtitleTrack { .. }
        ) {
            return self.apply_track(alias, action, ctx);
        }
        if !matches!(
            action,
            Action::Pause
                | Action::Resume
                | Action::ResumeMedia { .. }
                | Action::PlayItem { .. }
                | Action::SeekMedia { .. }
        ) {
            return Err(Failure::unsupported("unsupported"));
        }
        let (server, session) = self.session(alias, ctx)?;
        if matches!(action, Action::Pause | Action::Resume)
            && (session.now_playing_item.is_none() || session.play_state.is_none())
        {
            return Err(Failure::before("playback_state_unavailable"));
        }
        if matches!(action, Action::PlayItem { .. } | Action::SeekMedia { .. })
            && session.supports_media_control != Some(true)
        {
            return Err(Failure::unsupported("media_control_unsupported"));
        }
        // The Android TV legacy handler may toggle for Pause/Unpause. Read immediately before sending,
        // send once, and never retry an ambiguous response. External user races remain possible.
        let path = match action {
            Action::Pause if session.play_state.as_ref().is_some_and(|p| p.is_paused) => {
                return Ok(())
            }
            Action::Resume
                if session.play_state.as_ref().is_some_and(|p| !p.is_paused)
                    && session.now_playing_item.is_some() =>
            {
                return Ok(())
            }
            Action::Pause => format!("/Sessions/{}/Playing/Pause", encode(&session.id)),
            Action::Resume => format!("/Sessions/{}/Playing/Unpause", encode(&session.id)),
            Action::PlayItem {
                item_id,
                source_id,
                position_ms,
            }
            | Action::SeekMedia {
                item_id,
                source_id,
                position_ms,
            } => {
                let plan = plan.ok_or(Failure::before("missing_media_plan"))?;
                if &plan.item_id != item_id
                    || plan.source_id.as_ref() != Some(source_id)
                    || position_ms.checked_mul(MEDIA_TICKS_PER_MILLISECOND)
                        != Some(plan.position_ticks)
                {
                    return Err(Failure::before("media_plan_identity_mismatch"));
                }
                if matches!(action, Action::SeekMedia { .. }) {
                    let p = session
                        .play_state
                        .as_ref()
                        .ok_or(Failure::before("playback_state_unavailable"))?;
                    if session.now_playing_item.as_ref().map(|i| &i.id) != Some(item_id)
                        || p.media_source_id.as_ref() != Some(source_id)
                    {
                        return Err(Failure::before("seek_media_identity_mismatch"));
                    }
                    if p.can_seek != Some(true) {
                        return Err(Failure::unsupported("seek_unsupported"));
                    }
                    format!(
                        "/Sessions/{}/Playing/Seek?seekPositionTicks={}",
                        encode(&session.id),
                        plan.position_ticks
                    )
                } else {
                    let wanted = match plan.media_kind.as_deref() {
                        Some("audio") => "Audio",
                        Some("video") => "Video",
                        _ => return Err(Failure::unsupported("media_kind_unsupported")),
                    };
                    if !session.playable_media_types.iter().any(|t| t == wanted) {
                        return Err(Failure::unsupported("play_item_capability_unavailable"));
                    }
                    format!("/Sessions/{}/Playing?playCommand=PlayNow&itemIds={}&mediaSourceId={}&startPositionTicks={}", encode(&session.id), encode(item_id), encode(source_id), plan.position_ticks)
                }
            }
            Action::ResumeMedia { .. } => {
                let plan = plan.ok_or(Failure::before("missing_media_plan"))?;
                format!(
                    "/Sessions/{}/Playing?playCommand=PlayNow&itemIds={}&startPositionTicks={}",
                    encode(&session.id),
                    encode(&plan.item_id),
                    plan.position_ticks
                )
            }
            _ => return Err(Failure::unsupported("unsupported")),
        };
        ctx.check()?;
        let response = self.http.send(
            HttpRequest {
                server,
                method: "POST",
                path,
                mutating: true,
                json_body: None,
            },
            ctx,
        )?;
        if response.status != 204 {
            return Err(Failure::uncertain("jellyfin_command_not_confirmed"));
        }
        Ok(()) // 204 is only dispatch. The application use case verifies observed postconditions.
    }
}
pub fn patched_version(value: &str) -> bool {
    if value.contains('-') {
        return false;
    }
    let parts: Vec<_> = value.split('.').collect();
    (2..=4).contains(&parts.len())
        && parts.iter().all(|s| s.parse::<u32>().is_ok())
        && parts[0].parse::<u32>().is_ok_and(|major| major >= 12)
}
