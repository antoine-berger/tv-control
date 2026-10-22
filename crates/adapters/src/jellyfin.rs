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
            }) => Ok((server, user_id, device_id)),
            _ => Err(Failure::before("unsupported")),
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
            },
            ctx,
        )?;
        if response.status != 200 {
            return Err(Failure::before("jellyfin_read_failed"));
        }
        serde_json::from_slice(&response.body)
            .map_err(|_| Failure::before("jellyfin_invalid_response"))
    }
    fn session(&self, alias: &str, ctx: &Context<'_>) -> PortResult<(String, Session)> {
        let (server, user, device) = self.device(alias)?;
        let version: ServerInfo = self.get(server, "/System/Info/Public".into(), ctx)?;
        if !patched_version(&version.version) {
            return Err(Failure::before("jellyfin_server_requires_security_review"));
        }
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
            connected: true,
            active_app: None,
            playback,
            media_id: session.now_playing_item.map(|i| i.id),
            position_ticks: session.play_state.and_then(|p| p.position_ticks),
            volume_index: None,
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
        if !matches!(
            action,
            Action::Pause | Action::Resume | Action::ResumeMedia { .. }
        ) {
            return Err(Failure::before("unsupported"));
        }
        let (server, session) = self.session(alias, ctx)?;
        if matches!(action, Action::Pause | Action::Resume)
            && (session.now_playing_item.is_none() || session.play_state.is_none())
        {
            return Err(Failure::before("playback_state_unavailable"));
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
            Action::ResumeMedia { .. } => {
                let plan = plan.ok_or(Failure::before("missing_media_plan"))?;
                format!(
                    "/Sessions/{}/Playing?playCommand=PlayNow&itemIds={}&startPositionTicks={}",
                    encode(&session.id),
                    encode(&plan.item_id),
                    plan.position_ticks
                )
            }
            _ => return Err(Failure::before("unsupported")),
        };
        ctx.check()?;
        let response = self.http.send(
            HttpRequest {
                server,
                method: "POST",
                path,
                mutating: true,
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
