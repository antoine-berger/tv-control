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
