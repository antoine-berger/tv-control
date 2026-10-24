use super::*;
const PACKAGE: &str = "Packages:\n  Package [org.example.player] (abc):\n    versionCode=11207 minSdk=21\n    versionName=1.12.07\nHidden system packages:\n  Package [org.example.player] (def):\n    versionCode=1\n    versionName=old\n";
#[test]
fn app_details_are_fresh_bounded_and_read_only_without_mutation_permission() {
    let (adb, process, _) = adapter(
        false,
        &[
            VERSION,
            PACKAGE,
            "No activity found",
            PACKAGE,
            "org.example.player/.Search",
        ],
    );
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let first = adb
        .app_details("screen", &app(), &ctx(&clock, &cancel))
        .unwrap();
    assert_eq!(first.app.id, "org.example.player");
    assert_eq!(first.app.component, "org.example.player/.Main");
    assert_eq!(first.version.unwrap().name.as_deref(), Some("1.12.07"));
    assert_eq!(first.native_search, NativeSearch::Unsupported);
    let second = adb
        .app_details("screen", &app(), &ctx(&clock, &cancel))
        .unwrap();
    assert!(matches!(second.native_search, NativeSearch::Handler { .. }));
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 5);
    assert!(calls
        .iter()
        .all(|call| !call.mutating && call.stdin.is_empty()));
    for call in calls.iter().skip(1) {
        let command = call.args.last().unwrap().to_string_lossy();
        assert!(
            command.starts_with("'dumpsys' 'package'")
                || command.starts_with("'cmd' 'package' 'resolve-activity'")
        );
        assert!(command.contains("'org.example.player'"));
        assert!(!command.contains("VIEW"));
    }
}
#[test]
fn app_details_preserve_deadlines_and_cancellation_without_followup_probe() {
    let (adb, process, _) = adapter(false, &[VERSION, PACKAGE]);
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let context = ctx(&clock, &cancel);
    process.delay_next_ms.store(10_000, Ordering::Relaxed);
    assert_eq!(
        adb.app_details("screen", &app(), &context)
            .unwrap_err()
            .code,
        "expired"
    );
    assert_eq!(process.calls.lock().unwrap().len(), 1);
    assert!(process
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| !call.mutating));
}
#[test]
fn app_details_reject_invalid_identity_before_process_access() {
    let (adb, process, _) = adapter(false, &[]);
    let mut selected = app();
    selected.id = "org.other.app".into();
    assert_eq!(
        adb.app_details(
            "screen",
            &selected,
            &ctx(&TestClock::default(), &CancelFlag::default())
        )
        .unwrap_err()
        .code,
        "invalid_app_component"
    );
    assert!(process.calls.lock().unwrap().is_empty());
}
#[test]
fn app_details_do_not_misreport_read_failures_as_unsupported() {
    let (adb, process, _) = adapter(false, &[VERSION]);
    process.replies.lock().unwrap().extend([
        Err(Failure::before("adb_read_failed")),
        Err(Failure::before("adb_read_failed")),
    ]);
    let details = adb
        .app_details(
            "screen",
            &app(),
            &ctx(&TestClock::default(), &CancelFlag::default()),
        )
        .unwrap();
    assert_eq!(details.version, None);
    assert_eq!(details.native_search, NativeSearch::Unknown);
}
#[test]
fn app_details_enforce_output_bounds() {
    for (dump, handler) in [
        ("x".repeat(256 * 1024 + 1), "".into()),
        (PACKAGE.into(), "x".repeat(4097)),
    ] {
        let (adb, process, _) = adapter(false, &[VERSION, &dump, &handler]);
        assert_eq!(
            adb.app_details(
                "screen",
                &app(),
                &ctx(&TestClock::default(), &CancelFlag::default())
            )
            .unwrap_err()
            .code,
            "adb_output_limit"
        );
        assert!(process
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| !call.mutating));
    }
}

#[test]
fn cancelled_app_details_never_start_a_subprocess() {
    let (adb, process, _) = adapter(false, &[]);
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    cancel.cancel();
    assert_eq!(
        adb.app_details("screen", &app(), &ctx(&clock, &cancel))
            .unwrap_err()
            .code,
        "cancelled"
    );
    assert!(process.calls.lock().unwrap().is_empty());
}

#[test]
fn deadline_after_package_dump_prevents_search_resolution() {
    let (adb, process, _) = adapter(false, &[VERSION, PACKAGE, "No activity found", PACKAGE]);
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    adb.app_details("screen", &app(), &ctx(&clock, &cancel))
        .unwrap();
    let context = ctx(&clock, &cancel);
    process.delay_next_ms.store(10_000, Ordering::Relaxed);
    assert_eq!(
        adb.app_details("screen", &app(), &context)
            .unwrap_err()
            .code,
        "expired"
    );
    assert_eq!(process.calls.lock().unwrap().len(), 4);
}

#[test]
fn invalid_utf8_is_unknown_not_negative_capability_evidence() {
    let (adb, process, _) = adapter(false, &[VERSION]);
    process
        .replies
        .lock()
        .unwrap()
        .extend([Ok(vec![0xff]), Ok(vec![0xff])]);
    let details = adb
        .app_details(
            "screen",
            &app(),
            &ctx(&TestClock::default(), &CancelFlag::default()),
        )
        .unwrap();
    assert_eq!(details.version, None);
    assert_eq!(details.native_search, NativeSearch::Unknown);
}

#[test]
fn app_detail_read_preserves_ui_references_and_does_not_rewrite_search_configuration() {
    let (adb, process, _) = adapter(
        true,
        &[
            VERSION,
            UI,
            PACKAGE,
            "org.example.player/.NativeSearch",
            UI,
            "",
        ],
    );
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let context = ctx(&clock, &cancel);
    let view = adb.ui("screen", &context).unwrap();
    let mut selected = app();
    selected.search_component = None;
    let detail = adb.app_details("screen", &selected, &context).unwrap();
    assert!(matches!(detail.native_search, NativeSearch::Handler { .. }));
    assert_eq!(
        detail.app.search_component, None,
        "resolver evidence is not search configuration"
    );
    assert!(process
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| !call.mutating));
    adb.apply(
        "screen",
        &Intent::Activate {
            reference: view.elements[0].reference.clone(),
        },
        None,
        &context,
    )
    .unwrap();
    let calls = process.calls.lock().unwrap();
    let effects: Vec<_> = calls.iter().filter(|call| call.mutating).collect();
    assert_eq!(effects.len(), 1);
    assert_eq!(
        effects[0].args.last().unwrap(),
        "'sh' '-c' 'exec input \"$@\" 2>&1' 'tv-control-input' 'tap' '60' '40'"
    );
}
