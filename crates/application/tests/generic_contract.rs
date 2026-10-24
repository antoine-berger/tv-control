use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tv_application::{
    generic::{self, Control, Device, Journal, Reservation},
    Cancellation, Clock, Context,
};
use tv_domain::{generic::*, Failure, Observed, Playback, PlaybackEvidence, PortResult, Status};
#[derive(Default)]
struct Time(AtomicU64);
impl Clock for Time {
    fn unix_ms(&self) -> u64 {
        1000 + self.0.load(Ordering::SeqCst)
    }
    fn monotonic_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, n: u64) {
        self.0.fetch_add(n, Ordering::SeqCst);
    }
}
struct Never;
impl Cancellation for Never {
    fn is_cancelled(&self) -> bool {
        false
    }
}
struct Fake {
    apps: Vec<App>,
    state: Mutex<Observed>,
    effects: AtomicUsize,
    reads: AtomicUsize,
    authorized: bool,
    unknown: bool,
}
struct SharedFake(Arc<Fake>);
impl std::ops::Deref for SharedFake {
    type Target = Fake;
    fn deref(&self) -> &Fake {
        &self.0
    }
}
impl Device for SharedFake {
    fn resource(&self, _: &str) -> PortResult<String> {
        Ok("tv".into())
    }
    fn authorized(&self, _: &str) -> PortResult<()> {
        if self.authorized {
            Ok(())
        } else {
            Err(Failure::before("generic_control_not_authorized"))
        }
    }
    fn apps(&self, _: &str, _: &Context<'_>) -> PortResult<Vec<App>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.apps.clone())
    }
    fn observe(&self, _: &str, _: &Context<'_>) -> PortResult<Observed> {
        Ok(self.state.lock().unwrap().clone())
    }
    fn ui(&self, _: &str, _: &Context<'_>) -> PortResult<UiSnapshot> {
        Ok(UiSnapshot {
            revision: "v1".into(),
            truncation: vec![],
            focused_count: 0,
            elements: vec![],
        })
    }
    fn apply(
        &self,
        _: &str,
        intent: &Intent,
        app: Option<&App>,
        _: &Context<'_>,
    ) -> PortResult<()> {
        self.effects.fetch_add(1, Ordering::SeqCst);
        if self.unknown {
            return Err(Failure::uncertain("transport_lost"));
        }
        let mut s = self.state.lock().unwrap();
        match intent {
            Intent::OpenApp { .. } => s.active_app = Some(app.unwrap().id.clone()),
            Intent::Mute => s.volume_index = Some(0),
            Intent::SetVolume { index } => s.volume_index = Some(*index),
            Intent::Pause => s.playback = Playback::Paused,
            Intent::Resume => s.playback = Playback::Playing,
            _ => {}
        }
        Ok(())
    }
}
#[derive(Default)]
struct Memory {
    entries: Mutex<BTreeMap<String, (Request, Option<Reply>)>>,
    cancel: AtomicBool,
    fail_finish: AtomicBool,
}
impl Journal for Memory {
    fn reserve(&self, r: &Request, _: &str, _: u64) -> PortResult<Reservation> {
        let mut entries = self.entries.lock().unwrap();
        if let Some((old, reply)) = entries.get(&r.id) {
            return Ok(if old.device != r.device || old.intent != r.intent {
                Reservation::Conflict
            } else {
                Reservation::Existing(
                    reply
                        .clone()
                        .map(Operation::Complete)
                        .unwrap_or(Operation::Running),
                )
            });
        }
        entries.insert(r.id.clone(), (r.clone(), None));
        Ok(Reservation::Acquired)
    }
    fn finish(&self, r: &Request, reply: &Reply, _: u64) -> PortResult<()> {
        if self.fail_finish.load(Ordering::SeqCst) {
            return Err(Failure::before("disk_failed"));
        }
        self.entries.lock().unwrap().get_mut(&r.id).unwrap().1 = Some(reply.clone());
        Ok(())
    }
    fn get(&self, id: &str) -> PortResult<Operation> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .get(id)
            .map(|(_, r)| {
                r.clone()
                    .map(Operation::Complete)
                    .unwrap_or(Operation::Running)
            })
            .unwrap_or(Operation::Missing))
    }
    fn cancel(&self, id: &str) -> PortResult<Operation> {
        self.cancel.store(true, Ordering::SeqCst);
        self.get(id)
    }
    fn is_cancelled(&self, _: &str) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}
fn app(id: &str, label: Option<&str>) -> App {
    App {
        id: id.into(),
        component: format!("{id}/.Main"),
        label: label.map(Into::into),
        search_component: None,
    }
}
fn setup(apps: Vec<App>) -> (generic::Service<SharedFake, Time>, Arc<Fake>, Arc<Memory>) {
    let device = Arc::new(Fake {
        apps,
        state: Mutex::new(Observed {
            connected: true,
            playback_evidence: PlaybackEvidence::NativeState,
            ..Observed::default()
        }),
        effects: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
        authorized: true,
        unknown: false,
    });
    let journal = Arc::new(Memory::default());
    (
        generic::Service::new(
            SharedFake(device.clone()),
            journal.clone(),
            Time::default(),
            10000,
            10,
        ),
        device,
        journal,
    )
}
fn req(intent: Intent) -> Request {
    Request {
        id: "one".into(),
        device: "living".into(),
        intent,
        expires_at_unix_ms: 2000,
    }
}
#[test]
fn exact_label_resolves_and_replays_without_second_mutation() {
    let (s, d, _) = setup(vec![
        app("com.video", Some("YouTube")),
        app("com.music", Some("YouTube Music")),
    ]);
    let r = req(Intent::OpenApp {
        app: " youtube ".into(),
    });
    assert_eq!(s.execute(r.clone(), &Never).status, Status::Verified);
    assert!(s.execute(r, &Never).cached);
    assert_eq!(d.effects.load(Ordering::SeqCst), 1);
}
#[test]
fn duplicate_label_returns_choices_without_mutation() {
    let (s, d, _) = setup(vec![
        app("com.one", Some("Player")),
        app("com.two", Some("Player")),
    ]);
    let r = s.execute(
        req(Intent::OpenApp {
            app: "Player".into(),
        }),
        &Never,
    );
    assert_eq!(r.code, "app_ambiguous");
    assert_eq!(r.choices.len(), 2);
    assert_eq!(d.effects.load(Ordering::SeqCst), 0);
}
#[test]
fn missing_label_never_guesses_from_package() {
    let (s, d, _) = setup(vec![app("com.youtube", None)]);
    assert_eq!(
        s.execute(
            req(Intent::OpenApp {
                app: "youtube".into()
            }),
            &Never
        )
        .code,
        "catalogue_incomplete"
    );
    assert_eq!(d.effects.load(Ordering::SeqCst), 0);
}
#[test]
fn explicit_package_works_without_label() {
    let (s, _, _) = setup(vec![app("com.video", None)]);
    assert_eq!(
        s.execute(
            req(Intent::OpenApp {
                app: "com.video".into()
            }),
            &Never
        )
        .status,
        Status::Verified
    );
}
#[test]
fn search_without_advertised_handler_reports_dispatch_only() {
    let (s, d, _) = setup(vec![app("com.video", Some("Video"))]);
    assert_eq!(
        s.execute(
            req(Intent::Search {
                app: "Video".into(),
                query: "cats".into()
            }),
            &Never
        )
        .status,
        Status::Observed
    );
    assert_eq!(d.effects.load(Ordering::SeqCst), 1);
}
#[test]
fn navigation_does_not_claim_verified_playback() {
    let (s, _, _) = setup(vec![]);
    let r = s.execute(
        req(Intent::Navigate {
            keys: vec![Key::Home],
        }),
        &Never,
    );
    assert_eq!(r.status, Status::Observed);
    assert_eq!(r.code, "command_dispatched_state_observed");
}
#[test]
fn catalogue_is_bounded_cached_and_supports_unchanged_revision() {
    let (s, d, _) = setup(vec![app("com.video", Some("Video"))]);
    let read = |since| ReadRequest {
        device: "living".into(),
        expires_at_unix_ms: 2000,
        read: Read::Apps {
            query: None,
            since,
            limit: 10,
            refresh: false,
        },
    };
    let Data::Apps { revision, .. } = s.read(read(None), &Never).unwrap() else {
        panic!()
    };
    let Data::Apps {
        unchanged, apps, ..
    } = s.read(read(Some(revision)), &Never).unwrap()
    else {
        panic!()
    };
    assert!(unchanged && apps.is_empty());
    assert_eq!(d.reads.load(Ordering::SeqCst), 1);
}
#[test]
fn id_conflict_does_not_mutate() {
    let (s, d, _) = setup(vec![]);
    s.execute(req(Intent::Pause), &Never);
    assert_eq!(
        s.execute(req(Intent::Resume), &Never).code,
        "request_id_conflict"
    );
    assert_eq!(d.effects.load(Ordering::SeqCst), 1);
}
#[test]
fn failed_persistence_never_reports_success() {
    let (s, _, j) = setup(vec![]);
    j.fail_finish.store(true, Ordering::SeqCst);
    let r = s.execute(req(Intent::Pause), &Never);
    assert_eq!(r.status, Status::OutcomeUnknown);
    assert_eq!(r.code, "operation_persistence_failed");
    assert_eq!(s.execute(req(Intent::Pause), &Never).status, Status::Busy);
}
#[test]
fn malformed_and_expired_intents_do_not_mutate() {
    let (s, d, _) = setup(vec![]);
    assert_eq!(
        s.execute(req(Intent::Navigate { keys: vec![] }), &Never)
            .status,
        Status::Invalid
    );
    let mut r = req(Intent::Pause);
    r.expires_at_unix_ms = 1000;
    assert_eq!(s.execute(r, &Never).status, Status::Expired);
    assert_eq!(d.effects.load(Ordering::SeqCst), 0);
}
#[test]
fn cancelled_operation_never_mutates() {
    let (s, d, j) = setup(vec![]);
    j.cancel.store(true, Ordering::SeqCst);
    assert_eq!(
        s.execute(req(Intent::Pause), &Never).status,
        Status::Cancelled
    );
    assert_eq!(d.effects.load(Ordering::SeqCst), 0);
}

#[test]
fn explicit_component_dispatches_even_when_package_already_foreground() {
    let (service, device, _) = setup(vec![app("com.video", Some("Video"))]);
    device.state.lock().unwrap().active_app = Some("com.video".into());
    let result = service.execute(
        req(Intent::OpenApp {
            app: "com.video/.Main".into(),
        }),
        &Never,
    );
    assert_eq!(device.effects.load(Ordering::SeqCst), 1);
    assert_eq!(result.status, Status::Observed);
    assert_eq!(result.code, "command_dispatched_app_foreground");
}

#[test]
fn url_dispatch_never_claims_verified_content() {
    let (s, d, _) = setup(vec![app("com.video", Some("Video"))]);
    let r = s.execute(
        req(Intent::OpenUrl {
            app: "Video".into(),
            url: "https://video.example/watch?v=1".into(),
        }),
        &Never,
    );
    assert_eq!(r.status, Status::Observed);
    assert_eq!(r.code, "command_dispatched_state_observed");
    assert_eq!(d.effects.load(Ordering::SeqCst), 1);
}
#[test]
fn unsafe_urls_fail_before_any_effect() {
    for url in [
        "file:///etc/passwd",
        "intent://settings",
        "https://",
        "https://user:pass@example.org",
        "https://example.org/\nreboot",
        "https://example.org/a b",
    ] {
        let (s, d, _) = setup(vec![app("com.video", Some("Video"))]);
        assert_eq!(
            s.execute(
                req(Intent::OpenUrl {
                    app: "Video".into(),
                    url: url.into()
                }),
                &Never
            )
            .status,
            Status::Invalid
        );
        assert_eq!(d.effects.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn already_awake_is_verified_without_a_mutation() {
    let (s, d, _) = setup(vec![]);
    d.state.lock().unwrap().awake = Some(true);
    let result = s.execute(req(Intent::Wake), &Never);
    assert_eq!(result.status, Status::Verified);
    assert_eq!(d.effects.load(Ordering::SeqCst), 0);
}
#[test]
fn wake_unknown_power_is_only_dispatched_and_deduplicated() {
    let (s, d, _) = setup(vec![]);
    let result = s.execute(req(Intent::Wake), &Never);
    assert_eq!(result.status, Status::Observed);
    assert_eq!(result.code, "wake_dispatched_unverified");
    assert!(s.execute(req(Intent::Wake), &Never).cached);
    assert_eq!(d.effects.load(Ordering::SeqCst), 1);
}
#[test]
fn wake_still_asleep_expires_as_unknown_without_repeating() {
    let (s, d, _) = setup(vec![]);
    d.state.lock().unwrap().awake = Some(false);
    let result = s.execute(req(Intent::Wake), &Never);
    assert_eq!(result.status, Status::OutcomeUnknown);
    assert_eq!(d.effects.load(Ordering::SeqCst), 1);
    let mut retry = req(Intent::Wake);
    retry.expires_at_unix_ms = 3000;
    assert!(s.execute(retry, &Never).cached);
}

#[test]
fn mute_verifies_zero_and_repeated_request_never_mutates_again() {
    let (s, d, _) = setup(vec![]);
    d.state.lock().unwrap().volume_index = Some(8);
    let r = req(Intent::Mute);
    let result = s.execute(r.clone(), &Never);
    assert_eq!(result.status, Status::Verified);
    assert_eq!(result.observed.unwrap().volume_index, Some(0));
    assert!(s.execute(r, &Never).cached);
    let mut another = req(Intent::Mute);
    another.id = "another".into();
    assert_eq!(s.execute(another, &Never).status, Status::Verified);
    assert_eq!(d.effects.load(Ordering::SeqCst), 1);
}

#[test]
fn set_volume_verifies_and_conflicts_with_other_volume_intents() {
    let (s, d, _) = setup(vec![]);
    d.state.lock().unwrap().volume_index = Some(8);
    let r = req(Intent::SetVolume { index: 4 });
    let result = s.execute(r.clone(), &Never);
    assert_eq!(result.status, Status::Verified);
    assert_eq!(result.observed.unwrap().volume_index, Some(4));
    assert!(s.execute(r, &Never).cached);
    assert_eq!(
        s.execute(req(Intent::Mute), &Never).code,
        "request_id_conflict"
    );
    assert_eq!(d.effects.load(Ordering::SeqCst), 1);
}

#[test]
fn media_skip_is_observed_not_verified_and_duplicate_id_never_replays() {
    for command in [MediaCommand::Next, MediaCommand::Previous] {
        let (service, device, _) = setup(vec![]);
        device.state.lock().unwrap().playback = Playback::Playing;
        let request = req(Intent::Media {
            session: "org.example.player/player".into(),
            command,
        });
        let reply = service.execute(request.clone(), &Never);
        assert_eq!(reply.status, Status::Observed);
        assert_eq!(reply.code, "command_dispatched_state_observed");
        assert!(service.execute(request, &Never).cached);
        assert_eq!(device.effects.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn invalid_media_target_never_observes_or_mutates() {
    let (service, device, _) = setup(vec![]);
    for session in [
        "",
        "player",
        "org.example.player/player\nplay",
        "org.example.player/player/x",
    ] {
        assert_eq!(
            service
                .execute(
                    req(Intent::Media {
                        session: session.into(),
                        command: MediaCommand::Next
                    }),
                    &Never
                )
                .status,
            Status::Invalid
        );
    }
    assert_eq!(device.effects.load(Ordering::SeqCst), 0);
}
#[test]
fn media_pause_only_verifies_observed_native_paused_state() {
    let (service, device, _) = setup(vec![]);
    device.state.lock().unwrap().playback = Playback::Paused;
    let r = service.execute(
        req(Intent::Media {
            session: "org.example.player/player".into(),
            command: MediaCommand::Pause,
        }),
        &Never,
    );
    assert_eq!(r.status, Status::Verified);
    assert_eq!(device.effects.load(Ordering::SeqCst), 0);
}
#[test]
fn media_pause_without_native_evidence_cannot_verify() {
    let (service, device, _) = setup(vec![]);
    {
        let mut state = device.state.lock().unwrap();
        state.playback = Playback::Paused;
        state.playback_evidence = PlaybackEvidence::PositionProgressRequired;
    }
    let r = service.execute(
        req(Intent::Media {
            session: "org.example.player/player".into(),
            command: MediaCommand::Pause,
        }),
        &Never,
    );
    assert_eq!(r.status, Status::OutcomeUnknown);
    assert_eq!(device.effects.load(Ordering::SeqCst), 1);
}

#[test]
fn text_dispatch_is_observed_not_verified_and_deduplicated() {
    let (s, fake, _) = setup(vec![]);
    let request = req(Intent::TypeText {
        reference: "opaque:1".into(),
        text: "title".into(),
    });
    let first = s.execute(request.clone(), &Never);
    assert_eq!(first.status, Status::Observed);
    assert_eq!(first.code, "command_dispatched_state_observed");
    assert!(s.execute(request, &Never).cached);
    assert_eq!(fake.effects.load(Ordering::SeqCst), 1);
}
