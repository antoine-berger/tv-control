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
