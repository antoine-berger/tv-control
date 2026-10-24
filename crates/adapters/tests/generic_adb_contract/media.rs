use super::*;

#[test]
fn duplicate_media_sessions_cannot_dispatch() {
    let (adb, process, _) = adapter(
        true,
        &[
            VERSION,
            FOREGROUND,
            "Sessions:\n  tag=x, package=org.example.player\ntag=x, package=org.other.app",
        ],
    );
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    assert_eq!(
        adb.apply("screen", &Intent::Pause, None, &ctx(&clock, &flag))
            .unwrap_err()
            .code,
        "media_tag_ambiguous"
    );
    assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
}
const MEDIA_LIST: &str = "Sessions:\n  tag=player, package=org.example.player\n";
const MEDIA_DUMP: &str = include_str!("../fixtures/android/media-session.txt");
#[test]
fn media_read_exposes_only_matching_session_native_evidence() {
    let (adb, process, _) = adapter(true, &[VERSION, MEDIA_LIST, MEDIA_DUMP]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let sessions = adb.media("screen", &ctx(&clock, &flag)).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session, "org.example.player/player");
    assert_eq!(sessions[0].position_ms, Some(12000));
    assert_eq!(sessions[0].playback, tv_domain::Playback::Playing);
    let calls = process.calls.lock().unwrap();
    assert!(calls.iter().all(|call| !call.mutating));
}
#[test]
fn targeted_media_commands_send_only_typed_monitor_input() {
    for (command, input) in [
        (MediaCommand::Pause, "pause\nq\n"),
        (MediaCommand::Resume, "play\nq\n"),
        (MediaCommand::Next, "next\nq\n"),
        (MediaCommand::Previous, "previous\nq\n"),
    ] {
        let (adb, process, _) = adapter(true, &[VERSION, MEDIA_LIST, MEDIA_DUMP, "V2Monitoring session player... available commands: play, pause, next, previous\n(q)uit: finish monitoring\n"]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        adb.apply(
            "screen",
            &Intent::Media {
                session: "org.example.player/player".into(),
                command,
            },
            None,
            &ctx(&clock, &flag),
        )
        .unwrap();
        let calls = process.calls.lock().unwrap();
        let dispatched = calls
            .iter()
            .filter(|call| call.mutating)
            .collect::<Vec<_>>();
        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0].stdin, input.as_bytes());
        assert_eq!(
            dispatched[0].args.last().unwrap(),
            "'cmd' 'media_session' 'monitor' 'player'"
        );
    }
}
#[test]
fn scoped_media_rechecks_identity_and_refuses_before_dispatch() {
    for listing in ["Sessions:\n", "Sessions:\ntag=player, package=org.example.other\n", "Sessions:\ntag=player, package=org.example.player\ntag=player, package=org.example.player\n"] {
        let (adb, process, _) = adapter(true, &[VERSION, listing]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let error = adb.apply("screen", &Intent::Media { session: "org.example.player/player".into(), command: MediaCommand::Next }, None, &ctx(&clock, &flag)).unwrap_err();
        assert_eq!(error.effect, tv_domain::Effect::None);
        assert!(process.calls.lock().unwrap().iter().all(|call| !call.mutating));
    }
}
#[test]
fn scoped_media_monitor_success_without_session_banner_is_uncertain() {
    for output in [
        "",
        "No session found with id player",
        "V2Monitoring session player...\nInvalid command: next",
    ] {
        let (adb, _, _) = adapter(true, &[VERSION, MEDIA_LIST, MEDIA_DUMP, output]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let error = adb
            .apply(
                "screen",
                &Intent::Media {
                    session: "org.example.player/player".into(),
                    command: MediaCommand::Next,
                },
                None,
                &ctx(&clock, &flag),
            )
            .unwrap_err();
        assert_eq!(error.effect, tv_domain::Effect::MayHaveChanged);
        assert_eq!(error.code, "media_dispatch_unconfirmed");
    }
}
#[test]
fn media_permission_and_dump_limits_are_enforced() {
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let (adb, process, _) = adapter(false, &[]);
    assert_eq!(
        adb.apply(
            "screen",
            &Intent::Media {
                session: "org.example.player/player".into(),
                command: MediaCommand::Next
            },
            None,
            &ctx(&clock, &flag)
        )
        .unwrap_err()
        .code,
        "generic_control_not_authorized"
    );
    assert!(process.calls.lock().unwrap().is_empty());
    let oversized = "x".repeat(256 * 1024 + 1);
    let (adb, process, _) = adapter(true, &[VERSION, MEDIA_LIST, &oversized]);
    assert!(adb.media("screen", &ctx(&clock, &flag)).is_err());
    assert!(process
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| !call.mutating));
}

#[test]
fn known_unsupported_session_command_never_dispatches() {
    let zero = MEDIA_DUMP.replace("actions=55", "actions=0");
    for intent in [
        Intent::Media {
            session: "org.example.player/player".into(),
            command: MediaCommand::Pause,
        },
        Intent::Pause,
    ] {
        let replies = if intent == Intent::Pause {
            vec![VERSION, FOREGROUND, MEDIA_LIST, zero.as_str()]
        } else {
            vec![VERSION, MEDIA_LIST, zero.as_str()]
        };
        let (adb, process, _) = adapter(true, &replies);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        assert_eq!(
            adb.apply("screen", &intent, None, &ctx(&clock, &flag))
                .unwrap_err(),
            Failure::unsupported("media_command_unsupported")
        );
        assert!(process
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| !call.mutating));
    }
}
