//! Real durable journal, independent simulated devices and deterministic dispatch boundaries.
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};
use tv_adapters::{operation_journal::OperationJournal, runtime::CancelFlag};
use tv_application::{
    generic::{Control, Device, Journal, Service},
    Clock, Context,
};
use tv_domain::{
    generic::{App, Intent, Operation, Request, UiSnapshot},
    Failure, Observed, Playback, PlaybackEvidence, PortResult, Status,
};

const WAIT: Duration = Duration::from_secs(5);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "tv-multi-device-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn journal(&self) -> Arc<OperationJournal> {
        Arc::new(OperationJournal::open(&self.0.join("journal"), 16, 60_000).unwrap())
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// Scheduling waits do not consume a request's fake deadline. Unexpected polling
// advances time and expires rather than leaving a broken verification loop hung.
#[derive(Default)]
struct TestClock(AtomicU64);
impl Clock for TestClock {
    fn unix_ms(&self) -> u64 {
        100 + self.monotonic_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

struct Gate {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}
struct GateControl {
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}
fn gate() -> (Gate, GateControl) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    (
        Gate {
            entered: entered_tx,
            release: release_rx,
        },
        GateControl {
            entered: entered_rx,
            release: release_tx,
        },
    )
}
impl Gate {
    fn wait(self) -> PortResult<()> {
        self.entered
            .send(())
            .map_err(|_| Failure::before("fixture_controller_gone"))?;
        self.release
            .recv_timeout(WAIT)
            .map_err(|_| Failure::before("fixture_release_timeout"))
    }
}
impl GateControl {
    fn await_entry(&self) {
        self.entered.recv_timeout(WAIT).unwrap();
    }
    fn release(&self) {
        self.release.send(()).unwrap();
    }
}

#[derive(Clone, Copy)]
enum Boundary {
    BeforeObservation,
    AfterDispatch,
}
struct Endpoint {
    writes: AtomicUsize,
    boundary: Boundary,
    gate: Mutex<Option<Gate>>,
}
impl Endpoint {
    fn new(boundary: Boundary, gate: Option<Gate>) -> Arc<Self> {
        Arc::new(Self {
            writes: AtomicUsize::new(0),
            boundary,
            gate: Mutex::new(gate),
        })
    }
    fn wait_once(&self) -> PortResult<()> {
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.wait()?;
        }
        Ok(())
    }
    fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }
}
#[derive(Clone)]
struct Devices(BTreeMap<String, (String, Arc<Endpoint>)>);
impl Devices {
    fn endpoint(&self, device: &str) -> PortResult<&Endpoint> {
        self.0
            .get(device)
            .map(|(_, endpoint)| endpoint.as_ref())
            .ok_or(Failure::before("unknown_fixture_device"))
    }
}
impl Device for Devices {
    fn resource(&self, device: &str) -> PortResult<String> {
        self.0
            .get(device)
            .map(|(resource, _)| resource.clone())
            .ok_or(Failure::before("unknown_fixture_device"))
    }
    fn authorized(&self, device: &str) -> PortResult<()> {
        self.endpoint(device).map(|_| ())
    }
    fn apps(&self, _: &str, _: &Context<'_>) -> PortResult<Vec<App>> {
        Ok(vec![])
    }
    fn ui(&self, _: &str, _: &Context<'_>) -> PortResult<UiSnapshot> {
        Err(Failure::unsupported("fixture_ui_unsupported"))
    }
    fn observe(&self, device: &str, ctx: &Context<'_>) -> PortResult<Observed> {
        let endpoint = self.endpoint(device)?;
        if matches!(endpoint.boundary, Boundary::BeforeObservation) {
            endpoint.wait_once()?;
        }
        ctx.check()?;
        Ok(Observed {
            connected: true,
            playback: if endpoint.writes() == 0 {
                Playback::Playing
            } else {
                Playback::Paused
            },
            playback_evidence: PlaybackEvidence::NativeState,
            ..Observed::default()
        })
    }
    fn apply(
        &self,
        device: &str,
        _: &Intent,
        _: Option<&App>,
        ctx: &Context<'_>,
    ) -> PortResult<()> {
        let endpoint = self.endpoint(device)?;
        ctx.check()?;
        endpoint.writes.fetch_add(1, Ordering::SeqCst);
        if matches!(endpoint.boundary, Boundary::AfterDispatch) {
            endpoint.wait_once().map_err(|e| e.after_dispatch())?;
        }
        ctx.check().map_err(|e| e.after_dispatch())
    }
}
fn request(id: &str, device: &str) -> Request {
    Request {
        id: id.into(),
        device: device.into(),
        intent: Intent::Pause,
        expires_at_unix_ms: 2_000,
    }
}
fn service(devices: Devices, journal: Arc<OperationJournal>) -> Service<Devices, TestClock> {
    Service::new(devices, journal, TestClock::default(), 10_000, 5)
}

#[test]
fn cancellation_and_restart_are_isolated_at_both_dispatch_boundaries() {
    for boundary in [Boundary::BeforeObservation, Boundary::AfterDispatch] {
        let temp = Temp::new();
        let (gate_a, control_a) = gate();
        let (gate_b, control_b) = gate();
        let endpoint_a = Endpoint::new(boundary, Some(gate_a));
        let endpoint_b = Endpoint::new(Boundary::BeforeObservation, Some(gate_b));
        let devices = Devices(BTreeMap::from([
            ("alpha".into(), ("resource-a".into(), endpoint_a.clone())),
            (
                "alpha_alias".into(),
                ("resource-a".into(), endpoint_a.clone()),
            ),
            ("beta".into(), ("resource-b".into(), endpoint_b.clone())),
        ]));
        // Three independently opened handles exercise the durable shared state,
        // not a service-local mutex or a permissive in-memory journal mock.
        let journal_a = temp.journal();
        let journal_b = temp.journal();
        let observer = temp.journal();
        let a = service(devices.clone(), journal_a.clone());
        let b = service(devices.clone(), journal_b.clone());
        let alpha = request("request-a", "alpha");
        let beta = request("request-b", "beta");
        std::thread::scope(|scope| {
            let (done_a_tx, done_a_rx) = mpsc::channel();
            let alpha = alpha.clone();
            let handle_a = scope.spawn(move || {
                let reply = a.execute(alpha, &CancelFlag::default());
                done_a_tx.send(()).unwrap();
                reply
            });
            control_a.await_entry();
            let beta = beta.clone();
            let handle_b = scope.spawn(move || b.execute(beta, &CancelFlag::default()));
            control_b.await_entry();

            let competing = service(devices.clone(), temp.journal());
            for competing_request in [
                request("alias-request", "alpha_alias"),
                request("request-a", "alpha"),
            ] {
                let reply = competing.execute(competing_request, &CancelFlag::default());
                assert_eq!(reply.status, Status::Busy);
            }
            assert!(matches!(
                observer.cancel("request-a").unwrap(),
                Operation::Running
            ));
            assert!(journal_a.is_cancelled("request-a"));
            assert!(!journal_b.is_cancelled("request-b"));
            assert!(matches!(
                observer.get("request-b").unwrap(),
                Operation::Running
            ));

            control_b.release();
            let reply_b = handle_b.join().unwrap();
            assert_eq!(reply_b.status, Status::Verified, "{}", reply_b.code);
            assert_eq!(endpoint_b.writes(), 1);
            assert!(matches!(
                done_a_rx.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));

            control_a.release();
            let reply_a = handle_a.join().unwrap();
            let (expected, writes) = match boundary {
                Boundary::BeforeObservation => (Status::Cancelled, 0),
                Boundary::AfterDispatch => (Status::OutcomeUnknown, 1),
            };
            assert_eq!(reply_a.status, expected, "{}", reply_a.code);
            assert_eq!(endpoint_a.writes(), writes);
        });
        drop(journal_a);
        drop(journal_b);
        drop(observer);

        let restarted = service(devices, temp.journal());
        let expected_a = match boundary {
            Boundary::BeforeObservation => Status::Cancelled,
            Boundary::AfterDispatch => Status::OutcomeUnknown,
        };
        for (mut original, expected) in [(alpha, expected_a), (beta, Status::Verified)] {
            original.expires_at_unix_ms = 4_000;
            let replay = restarted.execute(original.clone(), &CancelFlag::default());
            assert!(replay.cached);
            assert_eq!(replay.status, expected);
            original.device = "alpha_alias".into();
            let conflict = restarted.execute(original, &CancelFlag::default());
            assert_eq!(conflict.code, "request_id_conflict");
        }
        assert_eq!(
            endpoint_a.writes(),
            usize::from(matches!(boundary, Boundary::AfterDispatch))
        );
        assert_eq!(endpoint_b.writes(), 1);
    }
}
