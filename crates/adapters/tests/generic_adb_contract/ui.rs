use super::*;

#[test]
fn stale_ui_ref_cannot_tap() {
    let changed = UI.replace("Search", "Purchase");
    let (adb, process, _) = adapter(true, &[VERSION, UI, &changed]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
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
        "ui_reference_stale"
    );
    assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
}
#[test]
fn fresh_ui_ref_is_consumed_and_expiry_cannot_dispatch() {
    let (adb, process, _) = adapter(true, &[VERSION, UI, UI, ""]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    let intent = Intent::Activate {
        reference: view.elements[0].reference.clone(),
    };
    adb.apply("screen", &intent, None, &ctx(&clock, &flag))
        .unwrap();
    assert_eq!(
        adb.apply("screen", &intent, None, &ctx(&clock, &flag))
            .unwrap_err()
            .code,
        "ui_reference_unknown"
    );
    assert_eq!(
        process
            .calls
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .args
            .last()
            .unwrap(),
        "'input' 'tap' '60' '40'"
    );
    let (adb, process, _) = adapter(true, &[VERSION, UI]);
    let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    clock.sleep_ms(30_001);
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
        "ui_reference_expired"
    );
    assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
}
#[test]
fn ui_dump_uses_private_temporary_file_and_removes_it() {
    let (adb, process, _) = adapter(false, &[VERSION, UI]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    let calls = process.calls.lock().unwrap();
    let command = calls[1].args.last().unwrap().to_str().unwrap();
    assert!(command.contains("umask 077"));
    assert!(command.contains("mktemp /data/local/tmp/tv-control-ui.XXXXXX"));
    assert!(command.contains("trap"));
    assert!(command.contains("rm -f"));
    assert!(!command.contains("/proc/self/fd/1"));
    assert!(!calls[1].mutating);
}
fn editable_ui() -> String {
    UI.replace("android.widget.Button", "android.widget.EditText")
        .replace("<node ", "<node package=\"org.example.player\" ")
}

#[test]
fn text_input_is_one_literal_argument_and_consumes_reference() {
    let ui = editable_ui();
    let (adb, process, _) = adapter(true, &[VERSION, &ui, &ui, FOREGROUND, ""]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    let action = Intent::TypeText {
        reference: view.elements[0].reference.clone(),
        text: "a b's $HOME `id`; & | \\".into(),
    };
    adb.apply("screen", &action, None, &ctx(&clock, &flag))
        .unwrap();
    assert_eq!(
        process
            .calls
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .args
            .last()
            .unwrap(),
        "'input' 'text' 'a b'\\''s $HOME `id`; & | \\'"
    );
    assert_eq!(
        adb.apply("screen", &action, None, &ctx(&clock, &flag))
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
            .filter(|c| c.mutating)
            .count(),
        1
    );
}

#[test]
fn unsupported_text_never_dispatches_or_consumes_reference() {
    let ui = editable_ui();
    let (adb, process, _) = adapter(true, &[VERSION, &ui]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    for text in ["%s", "%", "café", "中文", "a\nb", "a\0b", "\t", "", "   "] {
        assert!(
            adb.apply(
                "screen",
                &Intent::TypeText {
                    reference: view.elements[0].reference.clone(),
                    text: text.into()
                },
                None,
                &ctx(&clock, &flag)
            )
            .is_err(),
            "{text:?}"
        );
    }
    assert_eq!(process.calls.lock().unwrap().len(), 2);
}

#[test]
fn text_requires_unique_focused_editable_target() {
    let editable = editable_ui();
    for ui in [
        UI.to_string(),
        editable.replace("focused=\"true\"", "focused=\"false\""),
        editable.replace(
            "</hierarchy>",
            "<node password=\"true\" focused=\"true\"/></hierarchy>",
        ),
        editable.replace(
            "</hierarchy>",
            "<node focused=\"true\" bounds=\"[0,0][0,0]\"/></hierarchy>",
        ),
    ] {
        let (adb, process, _) = adapter(true, &[VERSION, &ui]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
        assert_eq!(
            adb.apply(
                "screen",
                &Intent::TypeText {
                    reference: view.elements[0].reference.clone(),
                    text: "title".into()
                },
                None,
                &ctx(&clock, &flag)
            )
            .unwrap_err()
            .code,
            "text_focus_not_editable"
        );
        assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
    }
}

#[test]
fn text_ref_rejects_changed_ui_foreground_and_unknown_package() {
    let ui = editable_ui();
    for (original, fresh, foreground, code) in [
        (
            ui.clone(),
            ui.replace("Search", "Other"),
            FOREGROUND,
            "ui_reference_stale",
        ),
        (
            ui.clone(),
            ui.clone(),
            "mResumedActivity: ActivityRecord{aa u0 org.other.app/.Main t1}\n",
            "text_focus_app_changed",
        ),
        (
            ui.replace(" package=\"org.example.player\"", ""),
            ui.replace(" package=\"org.example.player\"", ""),
            FOREGROUND,
            "text_focus_app_unknown",
        ),
    ] {
        let (adb, process, _) = adapter(true, &[VERSION, &original, &fresh, foreground]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
        assert_eq!(
            adb.apply(
                "screen",
                &Intent::TypeText {
                    reference: view.elements[0].reference.clone(),
                    text: "title".into()
                },
                None,
                &ctx(&clock, &flag)
            )
            .unwrap_err()
            .code,
            code
        );
        assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
    }
}

#[test]
fn navigation_invalidates_refs_and_plain_labels_cannot_be_tapped() {
    let (adb, _, _) = adapter(true, &[VERSION, UI, ""]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    adb.apply(
        "screen",
        &Intent::Navigate {
            keys: vec![Key::Back],
        },
        None,
        &ctx(&clock, &flag),
    )
    .unwrap();
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
    let label = UI.replace("clickable=\"true\"", "clickable=\"false\"");
    let (adb, process, _) = adapter(true, &[VERSION, &label]);
    let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
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
        "ui_element_not_actionable"
    );
    assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
}

#[test]
fn rereading_identical_ui_does_not_revive_old_references() {
    let (adb, process, _) = adapter(true, &[VERSION, UI, UI]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let first = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    let second = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    assert_eq!(first.revision, second.revision);
    assert_ne!(first.elements[0].reference, second.elements[0].reference);
    assert_eq!(
        adb.apply(
            "screen",
            &Intent::Activate {
                reference: first.elements[0].reference.clone()
            },
            None,
            &ctx(&clock, &flag)
        )
        .unwrap_err()
        .code,
        "ui_reference_unknown"
    );
    assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
}

#[test]
fn uncertain_text_dispatch_consumes_reference_without_retry() {
    let ui = editable_ui();
    let (adb, process, _) = adapter(true, &[VERSION, &ui, &ui, FOREGROUND]);
    process
        .replies
        .lock()
        .unwrap()
        .push_back(Err(Failure::uncertain("transport_lost")));
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    let action = Intent::TypeText {
        reference: view.elements[0].reference.clone(),
        text: "title".into(),
    };
    let error = adb
        .apply("screen", &action, None, &ctx(&clock, &flag))
        .unwrap_err();
    assert_eq!(error.effect, tv_domain::Effect::MayHaveChanged);
    assert_eq!(
        adb.apply("screen", &action, None, &ctx(&clock, &flag))
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
            .filter(|c| c.mutating)
            .count(),
        1
    );
}

#[test]
fn slow_revalidation_expires_reference_even_with_request_budget_remaining() {
    let ui = editable_ui();
    let (adb, process, _) = adapter(true, &[VERSION, &ui, &ui, FOREGROUND]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    process.delay_next_ms.store(30_001, Ordering::Relaxed);
    let context = Context {
        clock: &clock,
        cancellation: &flag,
        deadline_monotonic_ms: 100_000,
    };
    let error = adb
        .apply(
            "screen",
            &Intent::TypeText {
                reference: view.elements[0].reference.clone(),
                text: "title".into(),
            },
            None,
            &context,
        )
        .unwrap_err();
    assert_eq!(error.code, "ui_reference_expired");
    assert!(context.remaining_ms() > 0);
    assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
}

#[test]
fn concurrent_ui_read_invalidates_inflight_old_reference_before_dispatch() {
    struct RevalidationBarrier {
        inner: Arc<Mock>,
        count: std::sync::atomic::AtomicUsize,
        reached: std::sync::Barrier,
        resume: std::sync::Barrier,
    }
    impl Process for RevalidationBarrier {
        fn run(&self, call: Invocation, ctx: &Context<'_>) -> PortResult<Output> {
            let index = self.count.fetch_add(1, Ordering::SeqCst);
            let output = self.inner.run(call, ctx);
            if index == 2 {
                self.reached.wait();
                self.resume.wait();
            }
            output
        }
    }
    let mock = Arc::new(Mock::new(&[VERSION, UI, UI, UI]));
    let process = Arc::new(RevalidationBarrier {
        inner: mock.clone(),
        count: std::sync::atomic::AtomicUsize::new(0),
        reached: std::sync::Barrier::new(2),
        resume: std::sync::Barrier::new(2),
    });
    let adb = GenericAdb::new(
        Arc::new(cfg(true)),
        process.clone(),
        Arc::new(Probe(AtomicU32::new(41))),
    );
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let first = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    let intent = Intent::Activate {
        reference: first.elements[0].reference.clone(),
    };
    std::thread::scope(|scope| {
        let apply = scope.spawn(|| adb.apply("screen", &intent, None, &ctx(&clock, &flag)));
        process.reached.wait();
        let second = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
        assert_ne!(first.elements[0].reference, second.elements[0].reference);
        process.resume.wait();
        assert_eq!(
            apply.join().unwrap().unwrap_err().code,
            "ui_reference_unknown"
        );
    });
    assert!(mock.calls.lock().unwrap().iter().all(|c| !c.mutating));
}
