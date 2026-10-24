use super::*;

#[test]
fn mute_reads_native_zero_based_volume_without_app_or_calibration() {
    let (adb, process, _) = adapter(true, &[VERSION, "[V] volume is 8 in range [0..15]\n"]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let observed = adb
        .observe_for("screen", &Intent::Mute, &ctx(&clock, &flag))
        .unwrap();
    assert_eq!(observed.volume_index, Some(8));
    assert!(observed.connected);
    let calls = process.calls.lock().unwrap();
    assert!(calls.iter().all(|call| !call.mutating));
    assert_eq!(
        calls[1].args.last().unwrap(),
        "'cmd' 'media_session' 'volume' '--stream' '3' '--get'"
    );
}
#[test]
fn mute_never_toggles_or_increases_volume() {
    let (adb, process, _) = adapter(true, &[VERSION, ""]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    adb.apply("screen", &Intent::Mute, None, &ctx(&clock, &flag))
        .unwrap();
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].mutating);
    assert_eq!(
        calls[1].args.last().unwrap(),
        "'cmd' 'media_session' 'volume' '--stream' '3' '--set' '0'"
    );
}
#[test]
fn mute_rejects_unavailable_ambiguous_and_invalid_volume_evidence() {
    for output in [
        "unsupported",
        "volume is 3 in range [1..15]",
        "volume is 0 in range [0..0]",
        "volume is 4 in range [0..1001]",
        "volume is 4 in range [0..15]\nvolume is 0 in range [0..15]",
    ] {
        let (adb, process, _) = adapter(true, &[VERSION, output]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        assert_eq!(
            adb.observe_for("screen", &Intent::Mute, &ctx(&clock, &flag))
                .unwrap_err()
                .code,
            "volume_state_unavailable"
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
fn mute_requires_explicit_generic_consent() {
    let (adb, process, _) = adapter(false, &[]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    assert_eq!(
        adb.apply("screen", &Intent::Mute, None, &ctx(&clock, &flag))
            .unwrap_err()
            .code,
        "generic_control_not_authorized"
    );
    assert!(process.calls.lock().unwrap().is_empty());
}

#[test]
fn volume_setting_discovers_range_and_validates_before_dispatch() {
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let (adb, process, _) = adapter(true, &[VERSION, "[V] volume is 8 in range [0..15]", ""]);
    adb.apply(
        "screen",
        &Intent::SetVolume { index: 4 },
        None,
        &ctx(&clock, &flag),
    )
    .unwrap();
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(
        calls[2].args.last().unwrap(),
        "'cmd' 'media_session' 'volume' '--stream' '3' '--set' '4'"
    );
    drop(calls);
    let (adb, process, _) = adapter(true, &[VERSION, "[V] volume is 8 in range [0..15]"]);
    assert_eq!(
        adb.apply(
            "screen",
            &Intent::SetVolume { index: 16 },
            None,
            &ctx(&clock, &flag)
        )
        .unwrap_err()
        .code,
        "volume_out_of_range"
    );
    assert!(process
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| !call.mutating));
}
#[test]
fn volume_discovery_keeps_explicit_calibration_guard() {
    let mut config = cfg(true);
    if let tv_adapters::config::Device::Adb {
        volume_max_index, ..
    } = config.devices.get_mut("screen").unwrap()
    {
        *volume_max_index = Some(100);
    }
    let process = Arc::new(Mock::new(&[VERSION, "[V] volume is 8 in range [0..15]"]));
    let adb = GenericAdb::new(
        Arc::new(config),
        process.clone(),
        Arc::new(Probe(AtomicU32::new(41))),
    );
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    assert_eq!(
        adb.observe_for(
            "screen",
            &Intent::SetVolume { index: 4 },
            &ctx(&clock, &flag)
        )
        .unwrap_err()
        .code,
        "volume_calibration_changed"
    );
    assert!(process
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| !call.mutating));
}

#[test]
fn guarded_volume_target_checks_current_scale_and_dispatches_only_absolute_index() {
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let (adb, process, _) = adapter(true, &[VERSION, "[V] volume is 8 in range [0..15]", ""]);
    adb.set_volume_target("screen", 5, 15, &ctx(&clock, &flag))
        .unwrap();
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls[2].mutating);
    assert_eq!(
        calls[2].args.last().unwrap(),
        "'cmd' 'media_session' 'volume' '--stream' '3' '--set' '5'"
    );
    drop(calls);
    let (adb, process, _) = adapter(true, &[VERSION, "[V] volume is 8 in range [0..31]"]);
    let failure = adb
        .set_volume_target("screen", 5, 15, &ctx(&clock, &flag))
        .unwrap_err();
    assert_eq!(failure.code, "volume_scale_changed");
    assert!(process
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| !call.mutating));
}
#[test]
fn unresolved_relative_intents_cannot_be_sent_directly_to_adapter() {
    for intent in [
        Intent::AdjustVolume { delta: 1 },
        Intent::SetVolumePercent { percent: 20 },
    ] {
        let (adb, process, _) = adapter(true, &[]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let failure = adb
            .apply("screen", &intent, None, &ctx(&clock, &flag))
            .unwrap_err();
        assert_eq!(failure.code, "volume_target_unresolved");
        assert_eq!(failure.status(), tv_domain::Status::Unsupported);
        assert!(process.calls.lock().unwrap().is_empty());
    }
}
#[test]
fn relative_volume_observation_uses_native_range_without_foreground_or_media() {
    for intent in [
        Intent::AdjustVolume { delta: -1 },
        Intent::SetVolumePercent { percent: 50 },
    ] {
        let (adb, process, _) = adapter(true, &[VERSION, "[V] volume is 8 in range [0..15]"]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let state = adb
            .observe_for("screen", &intent, &ctx(&clock, &flag))
            .unwrap();
        assert_eq!(state.volume_index, Some(8));
        assert_eq!(state.volume_max_index, Some(15));
        let calls = process.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|call| !call.mutating));
    }
}

#[test]
fn resolved_volume_consumes_ui_refs_even_when_dispatch_is_uncertain() {
    let (adb, process, _) = adapter(true, &[VERSION, UI, "[V] volume is 8 in range [0..15]"]);
    process
        .replies
        .lock()
        .unwrap()
        .push_back(Err(Failure::uncertain("transport_lost")));
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    let error = adb
        .set_volume_target("screen", 5, 15, &ctx(&clock, &flag))
        .unwrap_err();
    assert_eq!(error.status(), tv_domain::Status::OutcomeUnknown);
    assert_eq!(
        adb.apply(
            "screen",
            &Intent::Activate {
                reference: view.elements[0].reference.clone()
            },
            None,
            &ctx(&clock, &flag)
        )
        .unwrap_err()
        .code,
        "ui_reference_unknown"
    );
    assert_eq!(
        process
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.mutating)
            .count(),
        1
    );
}
