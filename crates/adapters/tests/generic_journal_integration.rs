//! Exercise orchestration with the real durable journal rather than a permissive mock.
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
};
use tv_adapters::{
    operation_journal::OperationJournal,
    runtime::{CancelFlag, SystemClock},
};
use tv_application::{
    generic::{Control, Device, LegacyGuard, Service},
    Clock, Context,
};
use tv_domain::{
    generic::{App, Intent, Request, UiSnapshot},
    Failure, Observed, Playback, PlaybackEvidence, PortResult, Status,
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "tv-journal-integration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct FakeDevice(Arc<AtomicUsize>);
impl Device for FakeDevice {
    fn resource(&self, _: &str) -> PortResult<String> {
        Ok("physical-tv".into())
    }
    fn authorized(&self, _: &str) -> PortResult<()> {
        Ok(())
    }
    fn apps(&self, _: &str, _: &Context<'_>) -> PortResult<Vec<App>> {
        Ok(vec![])
    }
    fn observe(&self, _: &str, ctx: &Context<'_>) -> PortResult<Observed> {
        ctx.check()?;
        Ok(Observed {
            connected: true,
            playback: if self.0.load(Ordering::SeqCst) == 0 {
                Playback::Playing
            } else {
                Playback::Paused
            },
            playback_evidence: PlaybackEvidence::NativeState,
            ..Observed::default()
        })
    }
    fn ui(&self, _: &str, _: &Context<'_>) -> PortResult<UiSnapshot> {
        Err(Failure::before("unsupported"))
    }
    fn apply(&self, _: &str, _: &Intent, _: Option<&App>, ctx: &Context<'_>) -> PortResult<()> {
        ctx.check()?;
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn request(id: &str) -> Request {
    Request {
        id: id.into(),
        device: "living".into(),
        intent: Intent::Pause,
        expires_at_unix_ms: SystemClock::default().unix_ms() + 5000,
    }
}
#[test]
fn first_operation_executes_then_repeated_id_uses_durable_result() {
    let t = Temp::new();
    let journal = Arc::new(OperationJournal::open(&t.0.join("journal"), 16, 60000).unwrap());
    let applied = Arc::new(AtomicUsize::new(0));
    let service = Service::new(
        FakeDevice(applied.clone()),
        journal,
        SystemClock::default(),
        10000,
        5,
    );
    let result = service.execute(request("first"), &CancelFlag::default());
    assert_eq!(result.status, Status::Verified, "{}", result.code);
    assert_eq!(applied.load(Ordering::SeqCst), 1);
    let result = service.execute(request("first"), &CancelFlag::default());
    assert_eq!(result.status, Status::Verified);
    assert!(result.cached);
    assert_eq!(applied.load(Ordering::SeqCst), 1);
}
struct BlockingLegacy {
    ready: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl tv_application::Control for BlockingLegacy {
    fn execute(
        &self,
        request: tv_domain::Request,
        cancel: &dyn tv_application::Cancellation,
    ) -> tv_domain::Receipt {
        assert!(!cancel.is_cancelled());
        self.ready.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        tv_domain::Receipt {
            request_id: request.id,
            status: Status::Verified,
            code: "state_verified",
            observed: None,
            cached: false,
            local_elapsed_ms: 1,
        }
    }
}
#[test]
fn legacy_and_generic_paths_share_physical_device_exclusion() {
    let t = Temp::new();
    let journal = Arc::new(OperationJournal::open(&t.0.join("journal"), 16, 60000).unwrap());
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let legacy = LegacyGuard::new(
        Arc::new(BlockingLegacy {
            ready: ready_tx,
            release: Mutex::new(release_rx),
        }),
        journal.clone(),
        Arc::new(SystemClock::default()),
        BTreeMap::from([("legacy_alias".into(), "physical-tv".into())]),
        10000,
    );
    let handle = std::thread::spawn(move || {
        tv_application::Control::execute(
            &legacy,
            tv_domain::Request {
                id: "legacy".into(),
                device: "legacy_alias".into(),
                action: tv_domain::Action::Pause,
                expires_at_unix_ms: SystemClock::default().unix_ms() + 5000,
            },
            &CancelFlag::default(),
        )
    });
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();
    let applied = Arc::new(AtomicUsize::new(0));
    let service = Service::new(
        FakeDevice(applied.clone()),
        journal,
        SystemClock::default(),
        10000,
        5,
    );
    let blocked = service.execute(request("generic"), &CancelFlag::default());
    assert_eq!(blocked.status, Status::Busy);
    assert_eq!(applied.load(Ordering::SeqCst), 0);
    release_tx.send(()).unwrap();
    assert_eq!(handle.join().unwrap().status, Status::Verified);
    let result = service.execute(request("generic"), &CancelFlag::default());
    assert_eq!(result.status, Status::Verified, "{}", result.code);
    assert_eq!(applied.load(Ordering::SeqCst), 1);
}

struct CountingMedia(Arc<AtomicUsize>);
impl tv_application::Control for CountingMedia {
    fn execute(
        &self,
        request: tv_domain::Request,
        _: &dyn tv_application::Cancellation,
    ) -> tv_domain::Receipt {
        self.0.fetch_add(1, Ordering::SeqCst);
        tv_domain::Receipt {
            request_id: request.id,
            status: Status::Verified,
            code: "state_verified",
            observed: Some(Observed {
                media_id: Some("item".into()),
                media_source_id: Some("source".into()),
                connected: true,
                ..Default::default()
            }),
            cached: false,
            local_elapsed_ms: 1,
        }
    }
}
#[test]
fn media_action_fingerprint_and_source_evidence_survive_durable_restart() {
    use tv_application::Control as _;
    for seek in [false, true] {
        let t = Temp::new();
        let count = Arc::new(AtomicUsize::new(0));
        let guard = || {
            LegacyGuard::new(
                Arc::new(CountingMedia(count.clone())),
                Arc::new(OperationJournal::open(&t.0.join("journal"), 16, 60000).unwrap()),
                Arc::new(SystemClock::default()),
                BTreeMap::from([("living".into(), "physical-tv".into())]),
                10000,
            )
        };
        let action = |source: &str, position_ms| {
            if seek {
                tv_domain::Action::SeekMedia {
                    item_id: "item".into(),
                    source_id: source.into(),
                    position_ms,
                }
            } else {
                tv_domain::Action::PlayItem {
                    item_id: "item".into(),
                    source_id: source.into(),
                    position_ms,
                }
            }
        };
        let req = tv_domain::Request {
            id: "media".into(),
            device: "living".into(),
            action: action("source", 1000),
            expires_at_unix_ms: SystemClock::default().unix_ms() + 5000,
        };
        let first = guard();
        assert_eq!(
            first.execute(req.clone(), &CancelFlag::default()).status,
            Status::Verified
        );
        drop(first);
        let restarted = guard();
        let result = restarted.execute(req.clone(), &CancelFlag::default());
        assert!(result.cached);
        assert_eq!(
            result.observed.unwrap().media_source_id.as_deref(),
            Some("source")
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        for changed in [action("other", 1000), action("source", 2000)] {
            let mut r = req.clone();
            r.action = changed;
            assert_eq!(
                restarted.execute(r, &CancelFlag::default()).code,
                "request_id_conflict"
            );
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn remote_media_is_always_observed_and_durable_identity_prevents_replay() {
    use tv_domain::generic::RemoteMediaCommand;
    for command in [
        RemoteMediaCommand::Play,
        RemoteMediaCommand::Pause,
        RemoteMediaCommand::Stop,
        RemoteMediaCommand::Next,
        RemoteMediaCommand::Previous,
    ] {
        let t = Temp::new();
        let path = t.0.join("journal");
        let applied = Arc::new(AtomicUsize::new(1)); // Already paused must still dispatch an explicit key.
        let mut req = request("remote-key");
        req.intent = Intent::RemoteMedia { command };
        for cached in [false, true] {
            let journal = Arc::new(OperationJournal::open(&path, 16, 60000).unwrap());
            let service = Service::new(
                FakeDevice(applied.clone()),
                journal,
                SystemClock::default(),
                10000,
                5,
            );
            let result = service.execute(req.clone(), &CancelFlag::default());
            assert_eq!(result.status, Status::Observed);
            assert_eq!(result.code, "command_dispatched_state_observed");
            assert_eq!(result.cached, cached);
            assert_eq!(applied.load(Ordering::SeqCst), 2);
            let mut conflict = req.clone();
            conflict.intent = Intent::RemoteMedia {
                command: if command == RemoteMediaCommand::Stop {
                    RemoteMediaCommand::Play
                } else {
                    RemoteMediaCommand::Stop
                },
            };
            assert_eq!(
                service.execute(conflict, &CancelFlag::default()).code,
                "request_id_conflict"
            );
        }
    }
}
#[test]
fn remote_media_interrupted_journal_reservation_is_never_replayed() {
    use tv_application::generic::Journal;
    let t = Temp::new();
    let path = t.0.join("journal");
    let mut req = request("interrupted-key");
    req.intent = Intent::RemoteMedia {
        command: tv_domain::generic::RemoteMediaCommand::Next,
    };
    {
        let journal = OperationJournal::open(&path, 16, 60000).unwrap();
        assert!(matches!(
            journal
                .reserve(&req, "physical-tv", SystemClock::default().unix_ms())
                .unwrap(),
            tv_application::generic::Reservation::Acquired
        ));
    }
    let applied = Arc::new(AtomicUsize::new(0));
    let service = Service::new(
        FakeDevice(applied.clone()),
        Arc::new(OperationJournal::open(&path, 16, 60000).unwrap()),
        SystemClock::default(),
        10000,
        5,
    );
    let result = service.execute(req, &CancelFlag::default());
    assert_ne!(result.status, Status::Verified);
    assert_eq!(applied.load(Ordering::SeqCst), 0);
}

struct RefusingDevice {
    failure: Failure,
    observations: Arc<AtomicUsize>,
    dispatches: Arc<AtomicUsize>,
}
impl Device for RefusingDevice {
    fn resource(&self, _: &str) -> PortResult<String> {
        Ok("physical-tv".into())
    }
    fn authorized(&self, _: &str) -> PortResult<()> {
        Ok(())
    }
    fn apps(&self, _: &str, _: &Context<'_>) -> PortResult<Vec<App>> {
        Ok(vec![])
    }
    fn observe(&self, _: &str, _: &Context<'_>) -> PortResult<Observed> {
        self.observations.fetch_add(1, Ordering::SeqCst);
        Err(self.failure)
    }
    fn ui(&self, _: &str, _: &Context<'_>) -> PortResult<UiSnapshot> {
        Err(self.failure)
    }
    fn apply(&self, _: &str, _: &Intent, _: Option<&App>, _: &Context<'_>) -> PortResult<()> {
        self.dispatches.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn custom_failure_classification_survives_durable_restart_without_dispatch_or_reobservation() {
    for (failure, status) in [
        (
            Failure::unsupported("new_capability_refused"),
            Status::Unsupported,
        ),
        (Failure::expired("custom_deadline"), Status::Expired),
        (Failure::cancelled("custom_cancellation"), Status::Cancelled),
        (Failure::before("future_unsupported"), Status::Failed),
    ] {
        for uncertain in [false, true] {
            let t = Temp::new();
            let failure = if uncertain {
                failure.after_dispatch()
            } else {
                failure
            };
            let expected = if uncertain {
                Status::OutcomeUnknown
            } else {
                status
            };
            let observations = Arc::new(AtomicUsize::new(0));
            let dispatches = Arc::new(AtomicUsize::new(0));
            let req = request("classification");
            for restart in [false, true] {
                let journal =
                    Arc::new(OperationJournal::open(&t.0.join("journal"), 16, 60000).unwrap());
                let service = Service::new(
                    RefusingDevice {
                        failure,
                        observations: observations.clone(),
                        dispatches: dispatches.clone(),
                    },
                    journal,
                    SystemClock::default(),
                    10000,
                    5,
                );
                let result = service.execute(req.clone(), &CancelFlag::default());
                assert_eq!(result.status, expected);
                assert_eq!(result.code, failure.code);
                assert_eq!(result.cached, restart);
            }
            assert_eq!(observations.load(Ordering::SeqCst), 1);
            assert_eq!(dispatches.load(Ordering::SeqCst), 0);
        }
    }
}
