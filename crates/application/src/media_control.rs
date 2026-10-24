//! Shared validation and one-time resolution for identity-scoped media commands.
use tv_domain::{
    Action, Failure, MediaPlan, Observed, Playback, PortResult, MEDIA_TICKS_PER_MILLISECOND,
};

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
    // Without a reported rate/bounded trajectory or correlated command result,
    // natural playback progress cannot be distinguished from an ignored seek.
    if before.playback == Playback::Playing {
        return Err(Failure::unsupported("relative_seek_playing_unverifiable"));
    }
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

/// Observed paused displacement, never an inferred playing trajectory or causal proof.
pub(super) fn relative_verified(
    delta_ms: i64,
    before: &Observed,
    observed: &Observed,
    plan: &MediaPlan,
    tolerance_ms: u64,
) -> bool {
    if before.playback != Playback::Paused
        || observed.playback != Playback::Paused
        || !observed.connected
        || observed.media_id.as_ref() != Some(&plan.item_id)
        || observed.media_source_id != plan.source_id
    {
        return false;
    }
    let Some((initial, position)) = before.position_ticks.zip(observed.position_ticks) else {
        return false;
    };
    if (delta_ms > 0 && position <= initial) || (delta_ms < 0 && position >= initial) {
        return false;
    }
    let tolerance = tolerance_ms.saturating_mul(MEDIA_TICKS_PER_MILLISECOND);
    let displacement = delta_ms
        .unsigned_abs()
        .saturating_mul(MEDIA_TICKS_PER_MILLISECOND);
    // A broad tolerance must not swallow a small request. If native precision
    // cannot expose the exact target in this case, the outcome stays unknown.
    let precision = if displacement <= tolerance {
        0
    } else {
        tolerance
    };
    position.abs_diff(plan.position_ticks) <= precision
}
