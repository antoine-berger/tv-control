//! Bounded AOSP session listing/dump parsing. Native text is evidence, not commands.
use tv_domain::{generic::*, Failure, Playback, PortResult};

pub(crate) fn sessions(raw: &str) -> PortResult<Vec<(String, String)>> {
    let mut result = Vec::new();
    if !raw.lines().any(|line| line.trim() == "Sessions:") {
        return Err(Failure::before("media_sessions_unavailable"));
    }
    for line in raw.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("tag=") {
            let (tag, package) = rest
                .split_once(", package=")
                .ok_or(Failure::before("media_sessions_unavailable"))?;
            if result.len() == 64 || !valid_media_session(&format!("{package}/{tag}")) {
                return Err(Failure::before("media_sessions_unavailable"));
            }
            result.push((tag.to_owned(), package.to_owned()));
        } else if !line.is_empty() && line != "Sessions:" {
            return Err(Failure::before("media_sessions_unavailable"));
        }
    }
    Ok(result)
}

pub(crate) fn select<'a>(
    sessions: &'a [(String, String)],
    target: &str,
) -> PortResult<(&'a str, &'a str)> {
    if !valid_media_session(target) {
        return Err(Failure::before("invalid_media_session"));
    }
    let (package, tag) = target.split_once('/').expect("validated target");
    // Android monitor selects the first matching tag, not a package or token.
    let mut matches = sessions.iter().filter(|(t, _)| t == tag);
    let Some((tag, found_package)) = matches.next() else {
        return Err(Failure::before("media_session_not_found"));
    };
    if matches.next().is_some() {
        return Err(Failure::before("media_tag_ambiguous"));
    }
    if found_package != package {
        return Err(Failure::before("media_session_not_found"));
    }
    Ok((tag, found_package))
}

/// Resolve the only session for an implicit app request, then enforce globally
/// unique tags because Android monitor dispatches by tag, not package.
pub(crate) fn unique_for_package<'a>(
    sessions: &'a [(String, String)],
    package: &str,
) -> PortResult<&'a str> {
    let mut found = sessions.iter().filter(|(_, p)| p == package);
    let (tag, _) = found
        .next()
        .ok_or(Failure::before("media_session_not_found"))?;
    if found.next().is_some() {
        return Err(Failure::before("media_session_ambiguous"));
    }
    select(sessions, &format!("{package}/{tag}")).map(|(tag, _)| tag)
}

/// Native app playback must come from exactly one scoped session header. Flat
/// package/state text, inactive sessions and unsupported vendor dumps are not proof.
pub(crate) fn package_playback(raw: &str, package: &str) -> PortResult<Playback> {
    let mut tags = std::collections::BTreeSet::new();
    for line in raw.lines() {
        let mut fields = line.split_whitespace();
        let (Some(tag), Some(identity)) = (fields.next(), fields.next()) else {
            continue;
        };
        let target = format!("{package}/{tag}");
        if valid_media_session(&target)
            && identity
                .strip_prefix(&target)
                .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
        {
            tags.insert(tag);
        }
    }
    match tags.len() {
        0 => Ok(Playback::Unknown),
        1 => snapshot(raw, tags.first().expect("one tag"), package).map(|s| s.playback),
        _ => Err(Failure::before("media_session_ambiguous")),
    }
}

/// Missing native masks do not prove support or absence. A known mask can only
/// narrow the typed operation set; it never authorizes a new transport command.
pub(crate) fn require_command(state: &MediaSession, command: &MediaCommand) -> PortResult<()> {
    if state
        .supported_commands
        .as_ref()
        .is_some_and(|supported| !supported.contains(command))
    {
        return Err(Failure::unsupported("media_command_unsupported"));
    }
    Ok(())
}

pub(crate) fn snapshot(raw: &str, tag: &str, package: &str) -> PortResult<MediaSession> {
    let mut result = MediaSession {
        session: format!("{package}/{tag}"),
        package: package.into(),
        playback: Playback::Unknown,
        supported_commands: None,
        position_ms: None,
        description: None,
    };
    let prefix = format!("{tag} {package}/{tag}");
    let lines: Vec<_> = raw.lines().collect();
    let headers: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            line.trim_start()
                .strip_prefix(&prefix)
                .is_some_and(|rest| rest.starts_with('/') || rest.starts_with(" (userId="))
        })
        .collect();
    let [(index, header)] = headers.as_slice() else {
        return if headers.is_empty() {
            Ok(result)
        } else {
            Err(Failure::before("media_session_ambiguous"))
        };
    };
    let indent = header.len() - header.trim_start().len();
    let body: Vec<_> = lines[index + 1..]
        .iter()
        .take_while(|line| line.trim().is_empty() || line.len() - line.trim_start().len() > indent)
        .map(|line| line.trim())
        .collect();
    let field = |prefix: &str| -> Option<&str> {
        let mut values = body.iter().filter_map(|line| line.strip_prefix(prefix));
        let value = values.next()?;
        values.next().is_none().then_some(value)
    };
    if field("package=") != Some(package) {
        return Ok(result);
    }
    if let Some(state) = field("state=PlaybackState {").and_then(|s| s.strip_suffix('}')) {
        let number = |key: &str| -> Option<&str> {
            let mut values = state.split(',').filter_map(|s| s.trim().strip_prefix(key));
            let value = values.next()?;
            values.next().is_none().then_some(value)
        };
        // Android PlaybackState action constants, not app-specific guesses:
        // https://developer.android.com/reference/android/media/session/PlaybackState
        result.supported_commands =
            number("actions=")
                .and_then(|s| s.parse::<u64>().ok())
                .map(|mask| {
                    [
                        (2, MediaCommand::Pause),
                        (4, MediaCommand::Resume),
                        (32, MediaCommand::Next),
                        (16, MediaCommand::Previous),
                    ]
                    .into_iter()
                    .filter_map(|(bit, command)| (mask & bit != 0).then_some(command))
                    .collect()
                });
        if field("active=") != Some("true") {
            return Ok(result);
        }
        result.playback = match number("state=") {
            Some("0" | "1") => Playback::Stopped,
            Some("2") => Playback::Paused,
            Some("3")
                if number("speed=")
                    .and_then(|s| s.parse::<f32>().ok())
                    .is_some_and(|s| s.is_finite() && s > 0.0) =>
            {
                Playback::Playing
            }
            Some("6") => Playback::Buffering,
            _ => Playback::Unknown,
        };
        result.position_ms = number("position=")
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|n| n.try_into().ok());
    }
    if field("active=") != Some("true") {
        return Ok(result);
    }
    if let Some(description) = field("metadata: ").filter(|s| !s.is_empty() && *s != "null") {
        // Description may contain title/subtitle but not stable media ID or duration.
        result.description = Some(
            description
                .chars()
                .filter(|c| !c.is_control())
                .take(160)
                .collect(),
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    const DUMP: &str = include_str!("../../tests/fixtures/android/media-session.txt");
    #[test]
    fn aosp_snapshot_is_scoped_and_position_is_not_extrapolated() {
        let s = snapshot(DUMP, "player", "org.example.player").unwrap();
        assert_eq!(s.playback, Playback::Playing);
        assert_eq!(s.position_ms, Some(12000));
        assert_eq!(
            s.description.as_deref(),
            Some("Example title, Example artist, Example album")
        );
        assert_eq!(
            snapshot(DUMP, "other", "org.example.player")
                .unwrap()
                .playback,
            Playback::Unknown
        );
    }
    #[test]
    fn implicit_playback_requires_active_scoped_and_well_formed_native_evidence() {
        assert_eq!(
            package_playback(DUMP, "org.example.player").unwrap(),
            Playback::Playing
        );
        for replacement in [
            DUMP.replace("active=true", "active=false"),
            DUMP.replace("speed=1.0", "speed=1.0garbage"),
            DUMP.replace("speed=1.0", "speed=NaN"),
            DUMP.replace("    active=true", "    active=true\n    active=false"),
            "package=org.example.player\nstate=PlaybackState {state=3, speed=1.0}".into(),
            "  player org.example.player/player/7 (userId=0)\n    package=org.example.player\n    active=true\n  other org.other.player/other/7 (userId=0)\n    state=PlaybackState {state=3, speed=1.0}".into(),
        ] {
            assert_eq!(package_playback(&replacement, "org.example.player").unwrap(), Playback::Unknown);
        }
        assert_eq!(
            package_playback(&format!("{DUMP}{DUMP}"), "org.example.player")
                .unwrap_err()
                .code,
            "media_session_ambiguous"
        );
    }
    #[test]
    fn implicit_target_selection_uses_the_same_strict_listing_as_explicit_targets() {
        let listed = sessions("Sessions:\ntag=player, package=org.example.player\n").unwrap();
        assert_eq!(
            unique_for_package(&listed, "org.example.player").unwrap(),
            "player"
        );
        assert!(
            sessions("Sessions:\ntag=player, package=org.example.player\nunsupported row\n")
                .is_err()
        );
        let listed = sessions(
            "Sessions:\ntag=one, package=org.example.player\ntag=two, package=org.example.player\n",
        )
        .unwrap();
        assert_eq!(
            unique_for_package(&listed, "org.example.player")
                .unwrap_err()
                .code,
            "media_session_ambiguous"
        );
    }
    #[test]
    fn native_action_mask_zero_is_known_unsupported_not_unknown() {
        let zero = snapshot(
            &DUMP.replace("actions=55", "actions=0"),
            "player",
            "org.example.player",
        )
        .unwrap();
        assert_eq!(zero.supported_commands, Some(vec![]));
        let inactive = snapshot(
            &DUMP
                .replace("actions=55", "actions=0")
                .replace("active=true", "active=false"),
            "player",
            "org.example.player",
        )
        .unwrap();
        assert_eq!(inactive.playback, Playback::Unknown);
        assert_eq!(inactive.supported_commands, Some(vec![]));
        for command in [
            MediaCommand::Pause,
            MediaCommand::Resume,
            MediaCommand::Next,
            MediaCommand::Previous,
        ] {
            assert_eq!(
                require_command(&zero, &command).unwrap_err(),
                Failure::unsupported("media_command_unsupported")
            );
        }
        let unknown = snapshot(
            &DUMP.replace("actions=55", "vendorActions=55"),
            "player",
            "org.example.player",
        )
        .unwrap();
        assert_eq!(unknown.supported_commands, None);
        assert!(require_command(&unknown, &MediaCommand::Pause).is_ok());
        let pause = snapshot(
            &DUMP.replace("actions=55", "actions=2"),
            "player",
            "org.example.player",
        )
        .unwrap();
        assert_eq!(pause.supported_commands, Some(vec![MediaCommand::Pause]));
    }
    #[test]
    fn missing_duplicate_and_foreign_tags_never_select() {
        let s = sessions("Sessions:\n  tag=player, package=org.example.player\n  tag=player, package=org.example.other\n").unwrap();
        assert_eq!(
            select(&s, "org.example.player/player").unwrap_err().code,
            "media_tag_ambiguous"
        );
        let s = sessions("Sessions:\n  tag=player, package=org.example.other\n").unwrap();
        assert_eq!(
            select(&s, "org.example.player/player").unwrap_err().code,
            "media_session_not_found"
        );
        assert_eq!(
            select(&s, "org.example.other/missing").unwrap_err().code,
            "media_session_not_found"
        );
        assert!(select(&s, "org.example.other/player;reboot").is_err());
    }
    #[test]
    fn native_unknown_and_malformed_evidence_stays_unknown() {
        for position in ["-1", "18446744073709551615", "unknown"] {
            let s = snapshot(
                &DUMP.replace("position=12000", &format!("position={position}")),
                "player",
                "org.example.player",
            )
            .unwrap();
            assert_eq!(s.position_ms, None);
        }
        for speed in ["0.0", "NaN", "inf", "-1.0"] {
            assert_eq!(
                snapshot(
                    &DUMP.replace("speed=1.0", &format!("speed={speed}")),
                    "player",
                    "org.example.player"
                )
                .unwrap()
                .playback,
                Playback::Unknown
            );
        }
        assert!(snapshot(&format!("{DUMP}{DUMP}"), "player", "org.example.player").is_err());
        assert_eq!(
            snapshot(
                &DUMP.replace("package=org.example.player", "package=org.example.other"),
                "player",
                "org.example.player"
            )
            .unwrap()
            .playback,
            Playback::Unknown
        );
    }
    #[test]
    fn listing_is_bounded_and_does_not_hide_unsupported_rows() {
        assert!(sessions("Sessions:\n").unwrap().is_empty());
        for raw in [
            "unknown command",
            "Sessions:\ntag=x\n",
            "Sessions:\ntag=unsafe tag, package=org.example.player\n",
        ] {
            assert!(sessions(raw).is_err());
        }
        let huge = format!(
            "Sessions:\n{}",
            "tag=player, package=org.example.player\n".repeat(65)
        );
        assert!(sessions(&huge).is_err());
    }
    #[test]
    fn description_is_bounded_and_never_a_command_or_duration() {
        let s = snapshot(
            &DUMP.replace(
                "Example title, Example artist, Example album",
                &"a".repeat(500),
            ),
            "player",
            "org.example.player",
        )
        .unwrap();
        assert_eq!(s.description.unwrap().len(), 160);
    }
}
