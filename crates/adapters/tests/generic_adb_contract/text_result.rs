use super::*;

fn editor(text: &str) -> String {
    format!("<hierarchy><node resource-id=\"fixture:id/query\" package=\"org.example.player\" class=\"android.widget.EditText\" text=\"{text}\" enabled=\"true\" focused=\"true\" bounds=\"[10,20][110,60]\" /></hierarchy>")
}

#[test]
fn layout_mismatch_cursor_selection_and_partial_input_are_not_success_or_retried() {
    for (before, requested, actual) in [
        ("", "Tanya", "Tqnyq"),
        // TextView can expose an empty editor hint; dump omits that provenance.
        ("Search", "Tanya", "Tanya"),
        ("old", "new", "newold"),
        ("old", "new", "new"),
        ("", "Tanya", "Tan"),
        ("", "Tanya", ""),
    ] {
        let ui = editor(before);
        let after = editor(actual);
        let (adb, process, _) = adapter(
            true,
            &[VERSION, &ui, &ui, FOREGROUND, "", &after, FOREGROUND],
        );
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
        let action = Intent::TypeText {
            reference: view.elements[0].reference.clone(),
            text: requested.into(),
        };
        let error = adb
            .apply("screen", &action, None, &ctx(&clock, &flag))
            .unwrap_err();
        assert_eq!(error, Failure::uncertain("text_result_mismatch"));
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
}

#[test]
fn dumped_empty_text_is_not_content_description_and_append_preserves_entities_spaces() {
    for (before, requested, actual) in
        [("", "Tanya", "Tanya"), (" a&amp;b ", " x ", " a&amp;b  x ")]
    {
        let ui = editor(before).replace("<node ", "<node content-desc=\"Search\" ");
        let after = editor(actual);
        let (adb, _, _) = adapter(
            true,
            &[VERSION, &ui, &ui, FOREGROUND, "", &after, FOREGROUND],
        );
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
        adb.apply(
            "screen",
            &Intent::TypeText {
                reference: view.elements[0].reference.clone(),
                text: requested.into(),
            },
            None,
            &ctx(&clock, &flag),
        )
        .unwrap();
    }
}

#[test]
fn unreadable_ambiguous_or_unbounded_editor_refuses_before_dispatch() {
    let base = editor("");
    let duplicate = base.replace(
        "</hierarchy>",
        &base
            .replace("<hierarchy>", "")
            .replace("focused=\"true\"", "focused=\"false\""),
    );
    for ui in [
        base.replace("text=\"\" ", ""),
        base.replace("resource-id=\"fixture:id/query\" ", ""),
        base.replace("fixture:id/query", "fixture:id/qu?ry"),
        editor(&"x".repeat(257)),
        editor(&"x".repeat(255)),
        editor("a&#10;b"),
        editor("?"), // AOSP sanitizes Unicode/control to this ambiguous sentinel.
        editor("\u{00e9}"),
        base.replace("<node ", "<node showingHintText=\"true\" "),
        base.replace("<node ", "<node showing-hint-text=\"true\" "),
        duplicate,
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
                    text: "abc".into()
                },
                None,
                &ctx(&clock, &flag)
            )
            .unwrap_err(),
            Failure::before("text_verification_unsupported")
        );
        assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
    }
}

#[test]
fn post_dispatch_editor_and_foreground_changes_remain_uncertain() {
    let ui = editor("");
    for (after, foreground, code) in [
        (
            editor("Tanya").replace("fixture:id/query", "fixture:id/other"),
            FOREGROUND,
            "text_result_context_changed",
        ),
        (
            editor("Tanya").replace("focused=\"true\"", "focused=\"false\""),
            FOREGROUND,
            "text_result_context_changed",
        ),
        (
            editor("Tanya").replace("<node ", "<node password=\"true\" "),
            FOREGROUND,
            "text_result_context_changed",
        ),
        (editor("T?nya"), FOREGROUND, "text_result_unreadable"),
        (
            editor("Tanya").replace("<node ", "<node showingHintText=\"true\" "),
            FOREGROUND,
            "text_result_unreadable",
        ),
        (
            editor("Tanya").replace("text=\"Tanya\" ", ""),
            FOREGROUND,
            "text_result_unreadable",
        ),
        (
            editor("Tanya"),
            "mResumedActivity: ActivityRecord{aa u0 org.other.app/.Main t1}\n",
            "text_result_context_changed",
        ),
        ("not xml".into(), FOREGROUND, "ui_unavailable"),
    ] {
        let (adb, process, _) = adapter(
            true,
            &[VERSION, &ui, &ui, FOREGROUND, "", &after, foreground],
        );
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
        assert_eq!(
            adb.apply(
                "screen",
                &Intent::TypeText {
                    reference: view.elements[0].reference.clone(),
                    text: "Tanya".into()
                },
                None,
                &ctx(&clock, &flag)
            )
            .unwrap_err(),
            Failure::uncertain(code)
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
}

#[test]
fn deadline_or_cancellation_during_post_read_cannot_report_success() {
    struct DelayedRead {
        inner: Arc<Mock>,
        flag: Arc<CancelFlag>,
        cancel: bool,
    }
    impl Process for DelayedRead {
        fn run(&self, call: Invocation, ctx: &Context<'_>) -> PortResult<Output> {
            let result = self.inner.run(call, ctx);
            if self.inner.calls.lock().unwrap().len() == 6 {
                if self.cancel {
                    self.flag.cancel();
                } else {
                    ctx.clock.sleep_ms(10_001);
                }
            }
            result
        }
    }
    for cancel in [false, true] {
        let ui = editor("");
        let after = editor("Tanya");
        let mock = Arc::new(Mock::new(&[VERSION, &ui, &ui, FOREGROUND, "", &after]));
        let flag = Arc::new(CancelFlag::default());
        let adb = GenericAdb::new(
            Arc::new(cfg(true)),
            Arc::new(DelayedRead {
                inner: mock.clone(),
                flag: flag.clone(),
                cancel,
            }),
            Arc::new(Probe(AtomicU32::new(41))),
        );
        let clock = TestClock::default();
        let view = adb.ui("screen", &ctx(&clock, flag.as_ref())).unwrap();
        let error = adb
            .apply(
                "screen",
                &Intent::TypeText {
                    reference: view.elements[0].reference.clone(),
                    text: "Tanya".into(),
                },
                None,
                &ctx(&clock, flag.as_ref()),
            )
            .unwrap_err();
        assert_eq!(error.effect, tv_domain::Effect::MayHaveChanged);
        assert_eq!(
            mock.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.mutating)
                .count(),
            1
        );
    }
}
