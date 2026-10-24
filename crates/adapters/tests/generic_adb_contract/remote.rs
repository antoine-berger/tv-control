use super::*;

const INPUT_HELP: &str = "Usage: input [<source>] <command> [<arg>...]\n  keyevent [--longpress|--doubletap] <key code number or name> ... (Default: keyboard)\n";
#[test]
fn remote_capabilities_are_read_only_syntax_evidence_not_app_acceptance() {
    let (adb, process, _) = adapter(false, &[VERSION, INPUT_HELP, "30\n", FOREGROUND]);
    let caps = adb
        .remote_capabilities(
            "screen",
            &ctx(&TestClock::default(), &CancelFlag::default()),
        )
        .unwrap();
    assert_eq!(caps.android_sdk, Some(30));
    assert_eq!(caps.active_app.as_deref(), Some("org.example.player"));
    assert!(caps.keyevent_help && caps.longpress_help);
    let calls = process.calls.lock().unwrap();
    assert!(calls.iter().all(|c| !c.mutating));
    assert_eq!(calls[1].args.last().unwrap(), "'input'");
    assert_eq!(
        calls[2].args.last().unwrap(),
        "'getprop' 'ro.build.version.sdk'"
    );
}
#[test]
fn long_select_requires_fresh_help_and_has_only_one_closed_key() {
    let (adb, process, _) = adapter(
        true,
        &[
            VERSION,
            INPUT_HELP,
            "",
            "keyevent <key code number or name> ...\n",
        ],
    );
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    adb.apply("screen", &Intent::LongSelect, None, &ctx(&clock, &flag))
        .unwrap();
    assert_eq!(
        adb.apply("screen", &Intent::LongSelect, None, &ctx(&clock, &flag))
            .unwrap_err()
            .code,
        "unsupported"
    );
    let calls = process.calls.lock().unwrap();
    let effects: Vec<_> = calls.iter().filter(|c| c.mutating).collect();
    assert_eq!(effects.len(), 1);
    assert_eq!(
        effects[0].args.last().unwrap(),
        "'input' 'keyevent' '--longpress' '23'"
    );
}
#[test]
fn malformed_help_never_enables_long_select() {
    // Empty stdout also models older AOSP builds with usage only on stderr;
    // the process boundary deliberately does not expose stderr as evidence.
    for help in [
        "",
        "Error: keyevent --longpress unsupported",
        "text [--longpress] <key code number or name>",
        "keyevent [--longpress-extra] <key code number or name>",
        "keyevent [--longpress] <arbitrary>",
    ] {
        let (adb, process, _) = adapter(true, &[VERSION, help]);
        let e = adb
            .apply(
                "screen",
                &Intent::LongSelect,
                None,
                &ctx(&TestClock::default(), &CancelFlag::default()),
            )
            .unwrap_err();
        assert_eq!(e.code, "unsupported", "{help}");
        assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
    }
}
#[test]
fn new_navigation_keys_and_maximum_repeat_use_one_bounded_dispatch() {
    for (keys, expected) in [
        (
            vec![Key::Menu, Key::Settings],
            "'input' 'keyevent' '82' '176'".to_string(),
        ),
        (
            vec![Key::Down; 16],
            format!("'input' 'keyevent'{}", " '20'".repeat(16)),
        ),
    ] {
        let (adb, process, _) = adapter(true, &[VERSION, ""]);
        adb.apply(
            "screen",
            &Intent::Navigate { keys },
            None,
            &ctx(&TestClock::default(), &CancelFlag::default()),
        )
        .unwrap();
        let calls = process.calls.lock().unwrap();
        assert_eq!(calls.iter().filter(|c| c.mutating).count(), 1);
        assert_eq!(calls[1].args.last().unwrap().to_str().unwrap(), expected);
    }
    let (adb, process, _) = adapter(true, &[]);
    assert_eq!(
        adb.apply(
            "screen",
            &Intent::Navigate {
                keys: vec![Key::Down; 17]
            },
            None,
            &ctx(&TestClock::default(), &CancelFlag::default())
        )
        .unwrap_err()
        .code,
        "invalid_navigation"
    );
    assert!(process.calls.lock().unwrap().is_empty());
}
#[test]
fn long_select_expiry_and_authorization_prevent_even_discovery() {
    for granted in [false, true] {
        let (adb, process, _) = adapter(granted, &[]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let mut context = ctx(&clock, &flag);
        context.deadline_monotonic_ms = 0;
        assert!(adb
            .apply("screen", &Intent::LongSelect, None, &context)
            .is_err());
        assert!(process.calls.lock().unwrap().is_empty());
    }
}
#[test]
fn long_select_transport_loss_after_dispatch_is_uncertain_and_not_retried() {
    let (adb, process, _) = adapter(true, &[VERSION, INPUT_HELP]);
    process
        .replies
        .lock()
        .unwrap()
        .push_back(Err(Failure::uncertain("transport_lost")));
    let e = adb
        .apply(
            "screen",
            &Intent::LongSelect,
            None,
            &ctx(&TestClock::default(), &CancelFlag::default()),
        )
        .unwrap_err();
    assert_eq!(e.effect, tv_domain::Effect::MayHaveChanged);
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
fn long_select_deadline_after_probe_is_safe_but_after_dispatch_is_uncertain() {
    struct Advances {
        clock: Arc<TestClock>,
        expire_on_mutation: bool,
        effects: AtomicU32,
    }
    impl Process for Advances {
        fn run(&self, call: Invocation, ctx: &Context<'_>) -> PortResult<Output> {
            ctx.check()?;
            let output = if call.args == vec![std::ffi::OsString::from("version")] {
                VERSION
            } else if call.mutating {
                self.effects.fetch_add(1, Ordering::Relaxed);
                self.clock.sleep_ms(10_000);
                ""
            } else {
                if !self.expire_on_mutation {
                    self.clock.sleep_ms(10_000);
                }
                INPUT_HELP
            };
            Ok(Output {
                stdout: output.as_bytes().to_vec(),
                success: true,
            })
        }
    }
    for expire_on_mutation in [false, true] {
        let clock = Arc::new(TestClock::default());
        let process = Arc::new(Advances {
            clock: clock.clone(),
            expire_on_mutation,
            effects: AtomicU32::new(0),
        });
        let adb = GenericAdb::new(
            Arc::new(cfg(true)),
            process.clone(),
            Arc::new(Probe(AtomicU32::new(41))),
        );
        let flag = CancelFlag::default();
        let e = adb
            .apply("screen", &Intent::LongSelect, None, &ctx(&clock, &flag))
            .unwrap_err();
        assert_eq!(e.code, "expired");
        assert_eq!(
            process.effects.load(Ordering::Relaxed),
            u32::from(expire_on_mutation)
        );
        assert_eq!(
            e.effect,
            if expire_on_mutation {
                tv_domain::Effect::MayHaveChanged
            } else {
                tv_domain::Effect::None
            }
        );
    }
}

#[test]
fn long_select_invalidates_ui_reference_before_dispatch_even_when_help_fails() {
    for help in [INPUT_HELP, "unsupported syntax"] {
        let (adb, process, _) = adapter(true, &[VERSION, UI, help, ""]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
        let result = adb.apply("screen", &Intent::LongSelect, None, &ctx(&clock, &flag));
        assert_eq!(result.is_ok(), help == INPUT_HELP);
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
                adb.apply("screen", &intent, None, &ctx(&clock, &flag))
                    .unwrap_err()
                    .code,
                "ui_reference_unknown"
            );
        }
        assert_eq!(
            process.calls.lock().unwrap().len(),
            calls_before,
            "stale UI references must not read or dispatch after remote navigation"
        );
    }
}
