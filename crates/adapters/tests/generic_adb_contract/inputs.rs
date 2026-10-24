use super::*;
const BODY: &str = include_str!("../fixtures/tv_input.txt");
fn dump() -> String {
    format!("{BODY}\nTV_INPUT_DUMP_EXIT=0\n")
}
#[test]
fn input_discovery_is_fresh_read_only_and_does_not_need_mutation_consent() {
    let dump = dump();
    let second = dump.replace("; state: 0", "; state: 2");
    let (adb, process, _) = adapter(false, &[VERSION, &dump, &second]);
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let first = adb.inputs("screen", &ctx(&clock, &cancel)).unwrap();
    let second = adb.inputs("screen", &ctx(&clock, &cancel)).unwrap();
    assert_ne!(first, second);
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|c| !c.mutating && c.stdin.is_empty()));
    for call in calls.iter().skip(1) {
        assert_eq!(
            call.args.last().unwrap(),
            "'sh' '-c' 'dumpsys tv_input 2>&1; printf '\\''\nTV_INPUT_DUMP_EXIT=%s\n'\\'' \"$?\"'"
        );
    }
}
#[test]
fn input_discovery_propagates_transport_limits_and_deadline() {
    let oversized = "x".repeat(256 * 1024 + 65);
    let (adb, _, _) = adapter(false, &[VERSION, &oversized]);
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    assert_eq!(
        adb.inputs("screen", &ctx(&clock, &cancel))
            .unwrap_err()
            .code,
        "tv_inputs_dump_truncated"
    );
    let (adb, process, _) = adapter(false, &[VERSION, &dump()]);
    process.delay_next_ms.store(10_000, Ordering::Relaxed);
    assert_eq!(
        adb.inputs("screen", &ctx(&clock, &cancel))
            .unwrap_err()
            .code,
        "expired"
    );
    assert_eq!(process.calls.lock().unwrap().len(), 1);
}
#[test]
fn discovery_does_not_invalidate_an_existing_ui_reference() {
    let (adb, process, _) = adapter(true, &[VERSION, UI, &dump(), UI, ""]);
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let view = adb.ui("screen", &ctx(&clock, &cancel)).unwrap();
    adb.inputs("screen", &ctx(&clock, &cancel)).unwrap();
    assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
    adb.apply(
        "screen",
        &Intent::Activate {
            reference: view.elements[0].reference.clone(),
        },
        None,
        &ctx(&clock, &cancel),
    )
    .unwrap();
    assert_eq!(
        process
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.mutating)
            .count(),
        1
    );
}

#[test]
fn input_format_classification_does_not_confuse_truncation_or_dispatch_effects() {
    for (raw, code, status) in [
        (
            "unrecognized vendor format\n\nTV_INPUT_DUMP_EXIT=0\n",
            "tv_inputs_format_unsupported",
            tv_domain::Status::Unsupported,
        ),
        (
            "unrecognized vendor format",
            "tv_inputs_dump_truncated",
            tv_domain::Status::Failed,
        ),
    ] {
        let (adb, process, _) = adapter(false, &[VERSION, raw]);
        let error = adb
            .inputs(
                "screen",
                &ctx(&TestClock::default(), &CancelFlag::default()),
            )
            .unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(error.status(), status);
        assert_eq!(error.effect, tv_domain::Effect::None);
        assert!(process
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| !call.mutating));
    }
}
