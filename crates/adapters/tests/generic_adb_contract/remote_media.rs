use super::*;
const HELP: &str = "keyevent [--longpress] <key code number or name> ...\n";
fn intent(command: RemoteMediaCommand) -> Intent {
    Intent::RemoteMedia { command }
}
#[test]
fn dedicated_keys_are_single_system_global_dispatches() {
    for (command, key, sdk) in [
        (RemoteMediaCommand::Play, "126", "11"),
        (RemoteMediaCommand::Pause, "127", "11"),
        (RemoteMediaCommand::Stop, "86", "3"),
        (RemoteMediaCommand::Next, "87", "3"),
        (RemoteMediaCommand::Previous, "88", "3"),
    ] {
        let (adb, p, _) = adapter(true, &[VERSION, sdk, HELP, ""]);
        adb.apply(
            "screen",
            &intent(command),
            None,
            &ctx(&TestClock::default(), &CancelFlag::default()),
        )
        .unwrap();
        let calls = p.calls.lock().unwrap();
        let mutations: Vec<_> = calls.iter().filter(|c| c.mutating).collect();
        assert_eq!(mutations.len(), 1);
        assert_eq!(
            mutations[0].args.last().unwrap().to_str().unwrap(),
            format!("'sh' '-c' 'exec input \"$@\" 2>&1' 'tv-control-input' 'keyevent' '{key}'")
        );
        assert!(mutations[0].stdin.is_empty());
    }
}
#[test]
fn unsupported_sdk_or_help_never_injects() {
    for (sdk, help) in [
        ("10", HELP),
        ("unknown", HELP),
        ("30", "Error: keyevent unavailable"),
        ("30", ""),
    ] {
        let (adb, p, _) = adapter(true, &[VERSION, sdk, help]);
        let err = adb
            .apply(
                "screen",
                &intent(RemoteMediaCommand::Play),
                None,
                &ctx(&TestClock::default(), &CancelFlag::default()),
            )
            .unwrap_err();
        assert_eq!(err.code, "remote_media_unsupported");
        assert_eq!(err.status(), tv_domain::Status::Unsupported);
        assert_eq!(err.effect, tv_domain::Effect::None);
        assert!(p.calls.lock().unwrap().iter().all(|c| !c.mutating));
    }
}
#[test]
fn diagnostic_output_or_transport_loss_is_uncertain_without_replay() {
    for response in [
        Ok(b"Error: injection rejected".to_vec()),
        Err(Failure::uncertain("transport_lost")),
    ] {
        let (adb, p, _) = adapter(true, &[VERSION, "30", HELP]);
        p.replies.lock().unwrap().push_back(response);
        let err = adb
            .apply(
                "screen",
                &intent(RemoteMediaCommand::Stop),
                None,
                &ctx(&TestClock::default(), &CancelFlag::default()),
            )
            .unwrap_err();
        assert_eq!(err.effect, tv_domain::Effect::MayHaveChanged);
        assert_eq!(err.status(), tv_domain::Status::OutcomeUnknown);
        assert_eq!(
            p.calls
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
fn remote_media_observation_is_not_a_recipient_or_playback_claim() {
    for package in ["org.example.first", "org.example.second"] {
        let foreground = format!("mResumedActivity: ActivityRecord{{aa u0 {package}/.Main t1}}\n");
        let (adb, p, _) = adapter(false, &[VERSION, &foreground]);
        let observed = adb
            .observe_for(
                "screen",
                &intent(RemoteMediaCommand::Pause),
                &ctx(&TestClock::default(), &CancelFlag::default()),
            )
            .unwrap();
        assert_eq!(observed.active_app.as_deref(), Some(package));
        assert_eq!(observed.playback, tv_domain::Playback::Unknown);
        assert_eq!(
            observed.playback_evidence,
            tv_domain::PlaybackEvidence::PositionProgressRequired
        );
        assert!(p.calls.lock().unwrap().iter().all(|c| !c.mutating));
        assert_eq!(p.calls.lock().unwrap().len(), 2);
    }
}
#[test]
fn remote_media_invalidates_ui_references_even_if_probe_fails() {
    for sdk in ["30", "unknown"] {
        let (adb, p, _) = adapter(true, &[VERSION, UI, sdk, HELP, ""]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let context = ctx(&clock, &flag);
        let view = adb.ui("screen", &context).unwrap();
        let _ = adb.apply("screen", &intent(RemoteMediaCommand::Next), None, &context);
        let n = p.calls.lock().unwrap().len();
        assert_eq!(
            adb.apply(
                "screen",
                &Intent::Activate {
                    reference: view.elements[0].reference.clone()
                },
                None,
                &context
            )
            .unwrap_err()
            .code,
            "ui_reference_unknown"
        );
        assert_eq!(p.calls.lock().unwrap().len(), n);
    }
}
#[test]
fn remote_media_requires_authorization_and_live_deadline() {
    for granted in [false, true] {
        let (adb, p, _) = adapter(granted, &[]);
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        let mut context = ctx(&clock, &flag);
        if granted {
            context.deadline_monotonic_ms = 0;
        }
        assert!(adb
            .apply("screen", &intent(RemoteMediaCommand::Play), None, &context)
            .is_err());
        assert!(p.calls.lock().unwrap().is_empty());
    }
}

#[test]
fn remote_media_deadline_after_probe_is_safe_but_after_dispatch_is_uncertain() {
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
                if call
                    .args
                    .last()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .contains("getprop")
                {
                    "30"
                } else {
                    HELP
                }
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
            .apply(
                "screen",
                &intent(RemoteMediaCommand::Pause),
                None,
                &ctx(&clock, &flag),
            )
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
