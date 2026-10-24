//! Shared validation and one-time resolution for identity-scoped media commands.
use tv_domain::{Action, Failure, Observed, Playback, PortResult, MEDIA_TICKS_PER_MILLISECOND};

pub(super) fn valid(action: &Action) -> bool {
    let (item, source, value_valid) = match action {
        Action::PlayItem {
            item_id,
            source_id,
            position_ms,
        }
        | Action::SeekMedia {
            item_id,
            source_id,
            position_ms,
        } => (
            item_id,
            source_id,
            *position_ms <= i64::MAX as u64 / MEDIA_TICKS_PER_MILLISECOND,
        ),
        Action::SeekRelative {
            item_id,
            source_id,
            delta_ms,
        } => (
            item_id,
            source_id,
            *delta_ms != 0
                && delta_ms
                    .checked_mul(MEDIA_TICKS_PER_MILLISECOND as i64)
                    .is_some(),
        ),
        Action::SelectAudioTrack {
            item_id,
            source_id,
            index,
        } => (item_id, source_id, i32::try_from(*index).is_ok()),
        Action::SelectSubtitleTrack {
            item_id,
            source_id,
            index,
        } => (item_id, source_id, *index >= -1),
        _ => return true,
    };
    value_valid && crate::media::media_id(item) && crate::media::media_id(source)
}

pub(super) fn check_identity(action: &Action, state: &Observed) -> PortResult<()> {
    let (item, source) = match action {
        Action::SeekRelative {
            item_id, source_id, ..
        }
        | Action::SelectAudioTrack {
            item_id, source_id, ..
        }
        | Action::SelectSubtitleTrack {
            item_id, source_id, ..
        } => (item_id, source_id),
        _ => return Ok(()),
    };
    if state.media_id.as_ref() != Some(item)
        || state.media_source_id.as_ref() != Some(source)
        || !matches!(state.playback, Playback::Paused | Playback::Playing)
    {
        return Err(Failure::before("media_control_identity_mismatch"));
    }
    Ok(())
}

pub(super) fn resolve(action: &Action, before: &Observed) -> PortResult<Action> {
    let Action::SeekRelative {
        item_id,
        source_id,
        delta_ms,
    } = action
    else {
        return Ok(action.clone());
    };
    check_identity(action, before)?;
    // Public positions are milliseconds: discard only the sub-ms native fraction.
    let ticks = before
        .position_ticks
        .ok_or(Failure::unsupported("seek_position_unavailable"))?;
    if ticks > i64::MAX as u64 {
        return Err(Failure::before("invalid_media_position"));
    }
    let position = ticks / MEDIA_TICKS_PER_MILLISECOND;
    let target = i128::from(position) + i128::from(*delta_ms);
    if !(0..=i128::from(i64::MAX as u64 / MEDIA_TICKS_PER_MILLISECOND)).contains(&target) {
        return Err(Failure::before("media_position_out_of_range"));
    }
    Ok(Action::SeekMedia {
        item_id: item_id.clone(),
        source_id: source_id.clone(),
        position_ms: target as u64,
    })
}
