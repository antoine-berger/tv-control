//! Allowlisted Jellyfin DTOs. Paths, stream URLs and account details are not returned.
use super::*;
use tv_domain::media;

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Listing {
    items: Vec<CatalogueItem>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CatalogueItem {
    id: String,
    name: String,
    #[serde(rename = "Type")]
    kind: String,
    series_id: Option<String>,
    series_name: Option<String>,
    parent_index_number: Option<u32>,
    index_number: Option<u32>,
    run_time_ticks: Option<u64>,
    #[serde(default)]
    media_sources: Vec<Source>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Source {
    id: String,
    run_time_ticks: Option<u64>,
    #[serde(default)]
    media_streams: Vec<Track>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Track {
    index: u32,
    #[serde(rename = "Type")]
    kind: String,
    language: Option<String>,
    display_title: Option<String>,
    #[serde(default)]
    is_default: bool,
    #[serde(default)]
    is_forced: bool,
}
fn text(s: &str) -> bool {
    s.len() <= 2048 && !s.chars().any(|c| c.is_control() && c != '\n')
}
impl CatalogueItem {
    fn convert(self) -> PortResult<media::Item> {
        if !tv_application::media::media_id(&self.id)
            || !text(&self.name)
            || !text(&self.kind)
            || self
                .series_id
                .as_deref()
                .is_some_and(|s| !tv_application::media::media_id(s))
            || self.series_name.as_deref().is_some_and(|s| !text(s))
            || self.media_sources.len() > 16
        {
            return Err(Failure::before("jellyfin_invalid_media"));
        }
        let mut sources = Vec::new();
        for source in self.media_sources {
            if !tv_application::media::media_id(&source.id)
                || source.media_streams.len() > 128
                || sources.iter().any(|s: &media::Source| s.id == source.id)
            {
                return Err(Failure::before("jellyfin_invalid_media"));
            }
            let mut tracks = Vec::new();
            for t in source.media_streams {
                if !text(&t.kind)
                    || t.language.as_deref().is_some_and(|s| !text(s))
                    || t.display_title.as_deref().is_some_and(|s| !text(s))
                    || tracks.iter().any(|v: &media::Track| v.index == t.index)
                {
                    return Err(Failure::before("jellyfin_invalid_media"));
                }
                tracks.push(media::Track {
                    index: t.index,
                    kind: t.kind,
                    language: t.language,
                    title: t.display_title,
                    is_default: t.is_default,
                    is_forced: t.is_forced,
                });
            }
            sources.push(media::Source {
                id: source.id,
                duration_ticks: source.run_time_ticks,
                tracks,
            });
        }
        Ok(media::Item {
            id: self.id,
            name: self.name,
            kind: self.kind,
            series_id: self.series_id,
            series_name: self.series_name,
            season: self.parent_index_number,
            episode: self.index_number,
            duration_ticks: self.run_time_ticks,
            sources,
        })
    }
}
impl Jellyfin<'_> {
    pub fn prepare_media(
        &self,
        alias: &str,
        item_id: &str,
        source_id: &str,
        position_ms: u64,
        ctx: &Context<'_>,
    ) -> PortResult<MediaPlan> {
        let position_ticks = position_ms
            .checked_mul(MEDIA_TICKS_PER_MILLISECOND)
            .filter(|p| *p <= i64::MAX as u64)
            .ok_or(Failure::before("invalid_media_position"))?;
        let media::Data::Items { mut items, .. } = self.read_media(
            alias,
            &media::Read::Item {
                item_id: item_id.into(),
            },
            ctx,
        )?
        else {
            return Err(Failure::before("jellyfin_invalid_media"));
        };
        let item = items
            .pop()
            .ok_or(Failure::before("jellyfin_item_not_found"))?;
        if !matches!(
            item.kind.as_str(),
            "Movie" | "Episode" | "Audio" | "MusicVideo" | "Video"
        ) {
            return Err(Failure::unsupported("media_kind_unsupported"));
        }
        let source = item
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .ok_or(Failure::before("media_source_not_found"))?;
        if !source
            .duration_ticks
            .is_some_and(|duration| duration > 0 && position_ticks < duration)
        {
            return Err(Failure::before("media_position_out_of_range"));
        }
        Ok(MediaPlan {
            item_id: item.id,
            source_id: Some(source_id.into()),
            media_kind: Some(
                if item.kind == "Audio" {
                    "audio"
                } else {
                    "video"
                }
                .into(),
            ),
            position_ticks,
        })
    }

    pub fn read_media(
        &self,
        alias: &str,
        read: &media::Read,
        ctx: &Context<'_>,
    ) -> PortResult<media::Data> {
        if matches!(read, media::Read::Playback) {
            let (_, s) = self.session(alias, ctx)?;
            if s.client.as_deref().is_some_and(|s| !text(s))
                || s.application_version.as_deref().is_some_and(|s| !text(s))
                || s.supported_commands.len() > 128
                || s.supported_commands
                    .iter()
                    .any(|s| s.len() > 128 || !text(s))
                || s.now_playing_item
                    .as_ref()
                    .is_some_and(|i| !tv_application::media::media_id(&i.id))
                || s.play_state
                    .as_ref()
                    .and_then(|p| p.media_source_id.as_deref())
                    .is_some_and(|id| !tv_application::media::media_id(id))
                || s.play_state
                    .as_ref()
                    .and_then(|p| p.audio_stream_index)
                    .is_some_and(|n| n < 0)
                || s.play_state
                    .as_ref()
                    .and_then(|p| p.subtitle_stream_index)
                    .is_some_and(|n| n < -1)
            {
                return Err(Failure::before("jellyfin_invalid_playback_metadata"));
            }
            let capabilities = media::ControlCapabilities {
                seek: s.supports_media_control == Some(true)
                    && s.play_state
                        .as_ref()
                        .is_some_and(|p| p.can_seek == Some(true)),
                select_audio_track: s.supports_media_control == Some(true)
                    && s.supported_commands
                        .iter()
                        .any(|c| c == "SetAudioStreamIndex"),
                select_subtitle_track: s.supports_media_control == Some(true)
                    && s.supported_commands
                        .iter()
                        .any(|c| c == "SetSubtitleStreamIndex"),
            };
            let p = s.play_state;
            return Ok(media::Data::Playback(media::Playback {
                item_id: s.now_playing_item.map(|i| i.id),
                source_id: p.as_ref().and_then(|p| p.media_source_id.clone()),
                position_ticks: p.as_ref().and_then(|p| p.position_ticks),
                paused: p.as_ref().map(|p| p.is_paused),
                can_seek: p.as_ref().and_then(|p| p.can_seek),
                supports_media_control: s.supports_media_control,
                audio_index: p.as_ref().and_then(|p| p.audio_stream_index),
                subtitle_index: p.and_then(|p| p.subtitle_stream_index),
                client: s.client,
                client_version: s.application_version,
                supported_commands: s.supported_commands,
                capabilities,
            }));
        }
        let (server, user, _) = self.device(alias)?;
        self.check_version(server, ctx)?;
        let (path, limit) = match read {
            media::Read::Search { query, limit } => (format!("/Items?userId={}&searchTerm={}&recursive=true&includeItemTypes=Movie,Series,Episode,Audio,MusicAlbum&limit={}&enableImages=false&enableTotalRecordCount=false", encode(user), encode(query), u32::from(*limit) + 1), usize::from(*limit)),
            media::Read::Item { item_id } => (format!("/Items?userId={}&ids={}&fields=MediaSources,MediaStreams&limit=2&enableImages=false&enableTotalRecordCount=false", encode(user), encode(item_id)), 1),
            media::Read::Episodes { series_id, season, limit } => (format!("/Shows/{}/Episodes?userId={}&limit={}&isMissing=false&enableImages=false&enableUserData=false{}", encode(series_id), encode(user), u32::from(*limit) + 1, season.map(|n| format!("&season={n}")).unwrap_or_default()), usize::from(*limit)),
            media::Read::Playback => unreachable!(),
        };
        let list: Listing = self.get(server, path, ctx)?;
        if list.items.len() > limit + 1 {
            return Err(Failure::before("jellyfin_catalogue_limit"));
        }
        let truncated = list.items.len() > limit;
        let mut items = Vec::new();
        for item in list.items {
            let item = item.convert()?;
            if items.iter().any(|i: &media::Item| i.id == item.id) {
                return Err(Failure::before("jellyfin_duplicate_media"));
            }
            if let media::Read::Item { item_id } = read {
                if &item.id != item_id || truncated {
                    return Err(Failure::before("jellyfin_item_identity_mismatch"));
                }
            }
            if let media::Read::Episodes {
                series_id, season, ..
            } = read
            {
                if item.kind != "Episode"
                    || item.series_id.as_ref() != Some(series_id)
                    || season.is_some_and(|s| item.season != Some(s))
                {
                    return Err(Failure::before("jellyfin_episode_identity_mismatch"));
                }
            }
            items.push(item);
        }
        if matches!(read, media::Read::Item { .. }) && items.is_empty() {
            return Err(Failure::before("jellyfin_item_not_found"));
        }
        items.truncate(limit);
        Ok(media::Data::Items { items, truncated })
    }
}
