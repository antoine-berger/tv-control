use super::*;
const HELP: &str = "keyevent [--longpress] <key code number or name> ...";

#[test]
fn standby_qualifies_and_dispatches_only_dedicated_sleep_key() {
    let (adb, process, _) = adapter(
        true,
        &[
            VERSION,
            "mWakefulness=Awake",
            "20",
            HELP,
            "",
            "mWakefulness=Asleep",
        ],
    );
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let ctx = ctx(&clock, &cancel);
    assert_eq!(
        adb.observe_for("screen", &Intent::Standby, &ctx)
            .unwrap()
            .awake,
        Some(true)
    );
    adb.apply("screen", &Intent::Standby, None, &ctx).unwrap();
    assert_eq!(
        adb.observe_for("screen", &Intent::Standby, &ctx)
            .unwrap()
            .awake,
        Some(false)
    );
    let calls = process.calls.lock().unwrap();
    let mutations: Vec<_> = calls.iter().filter(|c| c.mutating).collect();
    assert_eq!(mutations.len(), 1);
    assert_eq!(
        mutations[0].args.last().unwrap(),
        "'sh' '-c' 'exec input \"$@\" 2>&1' 'tv-control-input' 'keyevent' '223'"
    );
    assert!(!calls
        .iter()
        .any(|c| c.args.last().unwrap().to_string_lossy().contains("volume")));
}
#[test]
fn standby_unknown_platform_or_input_support_never_mutates() {
    for replies in [
        vec![VERSION, "19"],
        vec![VERSION, "unknown"],
        vec![VERSION, "35", "unsupported"],
    ] {
        let (adb, process, _) = adapter(true, &replies);
        let clock = TestClock::default();
        let cancel = CancelFlag::default();
        assert_eq!(
            adb.apply("screen", &Intent::Standby, None, &ctx(&clock, &cancel))
                .unwrap_err()
                .code,
            "standby_unsupported"
        );
        assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
    }
}
#[test]
fn standby_power_read_is_readonly_and_needs_strong_asleep_evidence() {
    for raw in [
        "mWakefulness=Dreaming",
        "mWakefulness=Dozing",
        "mWakefulness=VendorUnknown",
        "mWakefulness=Asleep\nmWakefulness=Awake",
        "lastWakefulness=Asleep",
    ] {
        let (adb, process, _) = adapter(false, &[VERSION, raw]);
        let clock = TestClock::default();
        let cancel = CancelFlag::default();
        assert_eq!(
            adb.observe_for("screen", &Intent::Standby, &ctx(&clock, &cancel))
                .unwrap()
                .awake,
            None
        );
        assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
    }
}
#[test]
fn standby_requires_grant_and_live_deadline() {
    for granted in [false, true] {
        let (adb, process, _) = adapter(granted, &[]);
        let clock = TestClock::default();
        let cancel = CancelFlag::default();
        cancel.cancel();
        assert!(adb
            .apply("screen", &Intent::Standby, None, &ctx(&clock, &cancel))
            .is_err());
        assert!(process.calls.lock().unwrap().is_empty());
    }
    let (adb, process, _) = adapter(true, &[]);
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let mut ctx = ctx(&clock, &cancel);
    ctx.deadline_monotonic_ms = 0;
    assert!(adb.apply("screen", &Intent::Standby, None, &ctx).is_err());
    assert!(process.calls.lock().unwrap().is_empty());
}
#[test]
fn standby_qualification_deadline_never_dispatches() {
    let (adb, process, _) = adapter(true, &[VERSION]);
    process.delay_next_ms.store(10_001, Ordering::Relaxed);
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    assert!(adb
        .apply("screen", &Intent::Standby, None, &ctx(&clock, &cancel))
        .is_err());
    assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
}

#[test]
fn standby_dispatch_transport_fault_remains_uncertain() {
    let (adb, process, _) = adapter(true, &[VERSION, "35", HELP]);
    process
        .replies
        .lock()
        .unwrap()
        .push_back(Err(Failure::uncertain("transport_lost")));
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let error = adb
        .apply("screen", &Intent::Standby, None, &ctx(&clock, &cancel))
        .unwrap_err();
    assert_eq!(error.effect, tv_domain::Effect::MayHaveChanged);
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
fn standby_invalidates_activation_and_text_refs_even_when_platform_is_unsupported() {
    for sdk in ["35", "19"] {
        let (adb, process, _) = adapter(true, &[VERSION, UI, sdk, HELP, ""]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let context = ctx(&clock, &flag);
        let view = adb.ui("screen", &context).unwrap();
        assert_eq!(
            adb.apply("screen", &Intent::Standby, None, &context)
                .is_ok(),
            sdk == "35"
        );
        let calls_before = process.calls.lock().unwrap().len();
        for intent in [
            Intent::Activate {
                reference: view.elements[0].reference.clone(),
            },
            Intent::TypeText {
                reference: view.elements[0].reference.clone(),
                text: "query".into(),
            },
        ] {
            assert_eq!(
                adb.apply("screen", &intent, None, &context)
                    .unwrap_err()
                    .code,
                "ui_reference_unknown"
            );
        }
        assert_eq!(process.calls.lock().unwrap().len(), calls_before);
    }
}
