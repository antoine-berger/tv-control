//! Explicit identity-scoped GeneralCommand selection; no playback or track fallback.
//! Protocol: Jellyfin SessionController.SendFullGeneralCommand and GeneralCommand.Arguments.
use super::*;
use tv_domain::media;

fn selection(action: &Action) -> PortResult<(&str, &str, i32, &'static str, &'static str)> {
    match action {
        Action::SelectAudioTrack {
            item_id,
            source_id,
            index,
        } => Ok((
            item_id,
            source_id,
            i32::try_from(*index).map_err(|_| Failure::before("invalid_track_index"))?,
            "Audio",
            "SetAudioStreamIndex",
        )),
        Action::SelectSubtitleTrack {
            item_id,
            source_id,
            index,
        } if *index >= -1 => Ok((
            item_id,
            source_id,
            *index,
            "Subtitle",
            "SetSubtitleStreamIndex",
        )),
        _ => Err(Failure::before("invalid_track_selection")),
    }
}

fn active<'a>(
    session: &'a Session,
    item: &str,
    source: &str,
    command: &str,
) -> PortResult<&'a PlayState> {
    if session.supports_media_control != Some(true)
        || !session.supported_commands.iter().any(|s| s == command)
    {
        return Err(Failure::unsupported("track_selection_unsupported"));
    }
    let state = session
        .play_state
        .as_ref()
        .ok_or(Failure::before("playback_state_unavailable"))?;
    if session.now_playing_item.as_ref().map(|i| i.id.as_str()) != Some(item)
        || state.media_source_id.as_deref() != Some(source)
    {
        return Err(Failure::before("track_media_identity_mismatch"));
    }
    Ok(state)
}

impl Jellyfin<'_> {
    pub fn validate_track(
        &self,
        alias: &str,
        action: &Action,
        ctx: &Context<'_>,
    ) -> PortResult<()> {
        let (item, source, index, kind, command) = selection(action)?;
        if !tv_application::media::media_id(item) || !tv_application::media::media_id(source) {
            return Err(Failure::before("invalid_track_selection"));
        }
        let (_, session) = self.session(alias, ctx)?;
        active(&session, item, source, command)?;
        let media::Data::Items {
            items,
            truncated: false,
        } = self.read_media(
            alias,
            &media::Read::Item {
                item_id: item.into(),
            },
            ctx,
        )?
        else {
            return Err(Failure::before("track_metadata_unavailable"));
        };
        let source = items
            .iter()
            .find(|i| i.id == item)
            .and_then(|i| i.sources.iter().find(|s| s.id == source))
            .ok_or(Failure::before("media_source_not_found"))?;
        if index != -1
            && !source
                .tracks
                .iter()
                .any(|t| t.index == index as u32 && t.kind == kind)
        {
            return Err(Failure::before("track_index_not_found"));
        }
        ctx.check()
    }

    pub(super) fn apply_track(
        &self,
        alias: &str,
        action: &Action,
        ctx: &Context<'_>,
    ) -> PortResult<()> {
        self.validate_track(alias, action, ctx)?;
        let (item, source, index, _, command) = selection(action)?;
        // Revalidate current session identity and capabilities after catalogue I/O.
        let (server, session) = self.session(alias, ctx)?;
        let state = active(&session, item, source, command)?;
        ctx.check()?;
        let current = if command == "SetAudioStreamIndex" {
            state.audio_stream_index
        } else {
            state.subtitle_stream_index
        };
        if current == Some(index) {
            return Ok(());
        }
        let body =
            serde_json::json!({"Name":command,"Arguments":{"Index":index.to_string()}}).to_string();
        let response = self.http.send(
            HttpRequest {
                server,
                method: "POST",
                path: format!("/Sessions/{}/Command", encode(&session.id)),
                mutating: true,
                json_body: Some(body),
            },
            ctx,
        )?;
        if response.status != 204 {
            return Err(Failure::uncertain("jellyfin_command_not_confirmed"));
        }
        Ok(()) // Dispatch acknowledgement only; orchestration verifies track + paused state.
    }
}
