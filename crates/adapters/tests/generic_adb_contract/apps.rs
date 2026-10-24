use super::*;

#[test]
fn generic_grant_is_required_before_any_mutation() {
    let (adb, process, _) = adapter(false, &[]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let error = adb
        .apply(
            "screen",
            &Intent::OpenApp {
                app: "Example Player".into(),
            },
            Some(&app()),
            &ctx(&clock, &flag),
        )
        .unwrap_err();
    assert_eq!(error.code, "generic_control_not_authorized");
    assert!(process.calls.lock().unwrap().is_empty());
}
#[test]
fn version_cache_does_not_skip_live_protocol_guard() {
    let (adb, process, probe) = adapter(true, &[VERSION, FOREGROUND]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    adb.observe_for(
        "screen",
        &Intent::OpenApp {
            app: "Example Player".into(),
        },
        &ctx(&clock, &flag),
    )
    .unwrap();
    probe.0.store(40, Ordering::Relaxed);
    assert_eq!(
        adb.observe_for(
            "screen",
            &Intent::OpenApp {
                app: "Example Player".into()
            },
            &ctx(&clock, &flag)
        )
        .unwrap_err()
        .code,
        "adb_server_protocol_mismatch"
    );
    assert_eq!(process.calls.lock().unwrap().len(), 2);
}
#[test]
fn repeated_open_checks_do_not_read_unrelated_media_or_spawn_versions() {
    let (adb, process, _) = adapter(true, &[VERSION, FOREGROUND, FOREGROUND]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    for _ in 0..2 {
        let result = adb
            .observe_for(
                "screen",
                &Intent::OpenApp {
                    app: "Example Player".into(),
                },
                &ctx(&clock, &flag),
            )
            .unwrap();
        assert_eq!(result.active_app.as_deref(), Some("org.example.player"));
    }
    assert_eq!(process.calls.lock().unwrap().len(), 3);
}
#[test]
fn navigation_is_one_bounded_command() {
    let (adb, process, _) = adapter(true, &[VERSION, ""]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    adb.apply(
        "screen",
        &Intent::Navigate {
            keys: vec![Key::Down, Key::Down, Key::Select],
        },
        None,
        &ctx(&clock, &flag),
    )
    .unwrap();
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[1].args.last().unwrap(),
        "'sh' '-c' 'exec input \"$@\" 2>&1' 'tv-control-input' 'keyevent' '20' '20' '23'"
    );
    assert!(calls[1].mutating);
}
#[test]
fn search_query_is_one_quoted_argument_with_explicit_launcher_fallback() {
    let (adb, process, _) = adapter(true, &[VERSION, "Status: ok", "Status: ok"]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let query = "it's fun; reboot $(x)";
    adb.apply(
        "screen",
        &Intent::Search {
            app: "Example Player".into(),
            query: query.into(),
        },
        Some(&app()),
        &ctx(&clock, &flag),
    )
    .unwrap();
    let calls = process.calls.lock().unwrap();
    let command = calls[1].args.last().unwrap().to_str().unwrap();
    assert!(command.ends_with("'it'\\''s fun; reboot $(x)'"));
    drop(calls);
    let mut unsupported = app();
    unsupported.search_component = None;
    adb.apply(
        "screen",
        &Intent::Search {
            app: "Example Player".into(),
            query: "q".into(),
        },
        Some(&unsupported),
        &ctx(&clock, &flag),
    )
    .unwrap();
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls[2]
        .args
        .last()
        .unwrap()
        .to_str()
        .unwrap()
        .contains("'-n' 'org.example.player/.Main'"));
}
#[test]
fn discovered_apps_are_current_user_components_without_configuration_aliases() {
    let (adb, process, _) = adapter(
        false,
        &[VERSION, APP, "No activities found", "No activities found"],
    );
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let apps = adb.apps("screen", &ctx(&clock, &flag)).unwrap();
    assert_eq!(apps[0].label.as_deref(), Some("Example Player"));
    assert_eq!(apps[0].id, "org.example.player");
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert!(calls[1..].iter().all(|c| !c.mutating
        && c.args
            .last()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("'--user' 'current'")));
}
#[test]
fn url_is_targeted_to_discovered_package_and_quoted() {
    let (adb, process, _) = adapter(true, &[VERSION, APP, "Status: ok"]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let url = "https://video.example/watch?v=123&list=456";
    adb.apply(
        "screen",
        &Intent::OpenUrl {
            app: "Example Player".into(),
            url: url.into(),
        },
        Some(&app()),
        &ctx(&clock, &flag),
    )
    .unwrap();
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(!calls[1].mutating);
    let command = calls[2].args.last().unwrap().to_str().unwrap();
    assert!(command.contains("'-d' 'https://video.example/watch?v=123&list=456'"));
    assert!(command.ends_with("'-p' 'org.example.player'"));
    assert!(!command.contains("'-n'"));
}
#[test]
fn unsupported_url_never_dispatches() {
    let (adb, process, _) = adapter(true, &[VERSION, "No activities found"]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let result = adb.apply(
        "screen",
        &Intent::OpenUrl {
            app: "Example Player".into(),
            url: "https://video.example/watch".into(),
        },
        Some(&app()),
        &ctx(&clock, &flag),
    );
    assert_eq!(result.unwrap_err().code, "url_unsupported");
    assert!(process
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| !call.mutating));
}
