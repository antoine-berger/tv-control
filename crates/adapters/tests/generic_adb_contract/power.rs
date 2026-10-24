use super::*;

#[test]
fn wake_uses_dedicated_key_and_native_power_without_foreground() {
    let (adb, process, _) = adapter(
        true,
        &[VERSION, "mWakefulness=Asleep", "", "mWakefulness=Awake"],
    );
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let ctx = ctx(&clock, &flag);
    assert_eq!(
        adb.observe_for("screen", &Intent::Wake, &ctx)
            .unwrap()
            .awake,
        Some(false)
    );
    adb.apply("screen", &Intent::Wake, None, &ctx).unwrap();
    assert_eq!(
        adb.observe_for("screen", &Intent::Wake, &ctx)
            .unwrap()
            .awake,
        Some(true)
    );
    let calls = process.calls.lock().unwrap();
    assert_eq!(
        calls[2].args.last().unwrap(),
        "'sh' '-c' 'exec input \"$@\" 2>&1' 'tv-control-input' 'keyevent' '224'"
    );
    assert!(calls[2].mutating);
    assert!(!calls[1].mutating);
}
#[test]
fn wake_reads_complete_power_dump_beyond_the_text_budget() {
    // Vendor power dumps include large histories unrelated to current wakefulness.
    // Put the current field at the end too: accepting a truncated prefix is unsafe.
    let history = "  lastWakefulness=Awake, display=0, reason=fixture\n".repeat(3000);
    let asleep = format!("POWER MANAGER (dumpsys power)\n{history}  mWakefulness=Asleep\n");
    let awake = format!("POWER MANAGER (dumpsys power)\n{history}  mWakefulness=Awake\n");
    assert!(asleep.len() > 65_536);
    let process = Arc::new(Mock::new(&[VERSION, &asleep, "", &awake, &awake]));
    let mut config = cfg(true);
    config.preferences.max_output_bytes = 4096;
    let adb = GenericAdb::new(
        Arc::new(config),
        process.clone(),
        Arc::new(Probe(AtomicU32::new(41))),
    );
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let ctx = ctx(&clock, &flag);
    assert_eq!(
        adb.observe_for("screen", &Intent::Wake, &ctx)
            .unwrap()
            .awake,
        Some(false)
    );
    adb.apply("screen", &Intent::Wake, None, &ctx).unwrap();
    assert_eq!(
        adb.observe_for("screen", &Intent::Wake, &ctx)
            .unwrap()
            .awake,
        Some(true)
    );
    // The exception is specific to wake: ordinary reads keep the configured cap.
    assert_eq!(
        adb.observe("screen", &ctx).unwrap_err(),
        Failure::before("adb_output_limit")
    );
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 5);
    assert_eq!(calls[1].args.last().unwrap(), "'dumpsys' 'power'");
    assert_eq!(
        calls[2].args.last().unwrap(),
        "'sh' '-c' 'exec input \"$@\" 2>&1' 'tv-control-input' 'keyevent' '224'"
    );
    assert_eq!(calls.iter().filter(|call| call.mutating).count(), 1);
}
#[test]
fn wake_power_dump_limit_rejects_oversize_without_accepting_a_prefix() {
    let mut raw = "mWakefulness=Awake\n".to_string();
    raw.push_str(&" ".repeat(1024 * 1024 + 1 - raw.len()));
    let (adb, process, _) = adapter(true, &[VERSION, &raw]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let error = adb
        .observe_for("screen", &Intent::Wake, &ctx(&clock, &flag))
        .unwrap_err();
    assert_eq!(error, Failure::before("adb_output_limit"));
    assert!(process
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| !call.mutating));
}
#[test]
fn wake_power_dump_at_the_limit_keeps_trailing_ambiguity() {
    let first = "mWakefulness=Awake\n";
    let last = "\nmWakefulness=Asleep\n";
    let raw = format!(
        "{first}{}{last}",
        " ".repeat(1024 * 1024 - first.len() - last.len())
    );
    let (adb, _, _) = adapter(true, &[VERSION, &raw]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    assert_eq!(
        adb.observe_for("screen", &Intent::Wake, &ctx(&clock, &flag))
            .unwrap()
            .awake,
        None
    );
}
#[test]
fn wake_invalid_utf8_fails_before_dispatch() {
    let (adb, process, _) = adapter(true, &[VERSION]);
    process.replies.lock().unwrap().push_back(Ok(vec![0xff]));
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    assert_eq!(
        adb.observe_for("screen", &Intent::Wake, &ctx(&clock, &flag))
            .unwrap_err(),
        Failure::before("wake_transport_unavailable")
    );
    assert!(process
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| !call.mutating));
}
#[test]
fn wake_unknown_or_ambiguous_power_is_not_awake() {
    for raw in [
        "",
        "mWakefulness=VendorUnknown",
        "mWakefulness=Awake\nmWakefulness=Asleep",
        "lastWakefulness=Awake",
    ] {
        let (adb, _, _) = adapter(true, &[VERSION, raw]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        assert_eq!(
            adb.observe_for("screen", &Intent::Wake, &ctx(&clock, &flag))
                .unwrap()
                .awake,
            None
        );
    }
}
#[test]
fn wake_requires_grant() {
    let (adb, process, _) = adapter(false, &[]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    assert_eq!(
        adb.apply("screen", &Intent::Wake, None, &ctx(&clock, &flag))
            .unwrap_err()
            .code,
        "generic_control_not_authorized"
    );
    assert!(process.calls.lock().unwrap().is_empty());
}
