//! Last-observation diagnostics use the existing durable receipt schema.
use std::{
    collections::VecDeque,
    fs,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tv_adapters::operation_journal::OperationJournal;
use tv_application::{
    generic::{Control, Device, Service},
    Cancellation, Clock, Context,
};
use tv_domain::{generic::*, Failure, Observed, Playback, PlaybackEvidence, PortResult, Status};

#[derive(Clone, Default)]
struct Time(Arc<AtomicU64>);
impl Clock for Time {
    fn unix_ms(&self) -> u64 {
        1000 + self.monotonic_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, n: u64) {
        self.0.fetch_add(n, Ordering::SeqCst);
    }
}
#[derive(Default)]
struct Cancel(AtomicBool);
impl Cancellation for Cancel {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
enum Sample {
    State(Observed),
    Error,
    Late,
    Cancel,
}
struct Fake {
    samples: Mutex<VecDeque<Sample>>,
    effects: Arc<AtomicUsize>,
    cancel: Arc<Cancel>,
    apply_error: bool,
}
impl Device for Fake {
    fn resource(&self, _: &str) -> PortResult<String> {
        Ok("physical-tv".into())
    }
    fn authorized(&self, _: &str) -> PortResult<()> {
        Ok(())
    }
    fn apps(&self, _: &str, _: &Context<'_>) -> PortResult<Vec<App>> {
        Ok(vec![App {
            id: "org.example.music".into(),
            component: "org.example.music/.Launcher".into(),
            label: Some("Music".into()),
            search_component: None,
        }])
    }
    fn observe(&self, _: &str, ctx: &Context<'_>) -> PortResult<Observed> {
        ctx.check()?;
        match self
            .samples
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Sample::State(state("org.example.video")))
        {
            Sample::State(s) => Ok(s),
            Sample::Error => Err(Failure::before("transport_lost")),
            Sample::Late => {
                ctx.clock.sleep_ms(ctx.remaining_ms());
                Ok(state("org.example.music"))
            }
            Sample::Cancel => {
                self.cancel.0.store(true, Ordering::SeqCst);
                Ok(state("org.example.music"))
            }
        }
    }
    fn ui(&self, _: &str, _: &Context<'_>) -> PortResult<UiSnapshot> {
        Err(Failure::before("unsupported"))
    }
    fn apply(&self, _: &str, _: &Intent, _: Option<&App>, ctx: &Context<'_>) -> PortResult<()> {
        ctx.check()?;
        self.effects.fetch_add(1, Ordering::SeqCst);
        if self.apply_error {
            Err(Failure::uncertain("transport_lost"))
        } else {
            Ok(())
        }
    }
}
fn state(package: &str) -> Observed {
    Observed {
        connected: true,
        active_app: Some(package.into()),
        playback: Playback::Playing,
        playback_evidence: PlaybackEvidence::NativeState,
        media_id: Some("private-title-or-url".into()),
        media_source_id: Some("private-source-id".into()),
        position_ticks: Some(12345),
        ..Observed::default()
    }
}
struct Fixture {
    service: Service<Fake, Time>,
    effects: Arc<AtomicUsize>,
    cancel: Arc<Cancel>,
    path: std::path::PathBuf,
}
impl Fixture {
    fn new(samples: Vec<Sample>, apply_error: bool) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "tv-observation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&path).unwrap();
        let journal = Arc::new(OperationJournal::open(&path.join("journal"), 16, 60000).unwrap());
        let effects = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(Cancel::default());
        let fake = Fake {
            samples: Mutex::new(samples.into()),
            effects: effects.clone(),
            cancel: cancel.clone(),
            apply_error,
        };
        Self {
            service: Service::new(fake, journal, Time::default(), 1000, 10),
            effects,
            cancel,
            path,
        }
    }
    fn execute(&self) -> Reply {
        self.service.execute(request(), &*self.cancel)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
fn request() -> Request {
    Request {
        id: "launch".into(),
        device: "living".into(),
        intent: Intent::OpenApp {
            app: "org.example.music/.Launcher".into(),
        },
        expires_at_unix_ms: 1100,
    }
}
fn assert_unknown(r: &Reply, code: &str) {
    assert_eq!(r.status, Status::OutcomeUnknown);
    assert_eq!(r.code, code);
    let s = r.observed.as_ref().unwrap();
    assert_eq!(s.active_app.as_deref(), Some("org.example.video"));
    assert!(s.connected);
    assert_eq!(s.playback, Playback::Playing);
    assert_eq!(s.media_id, None);
    assert_eq!(s.media_source_id, None);
    assert_eq!(s.position_ticks, None);
}
#[test]
fn cross_package_launch_is_unknown_with_bounded_durable_evidence_not_redirect_success() {
    let f = Fixture::new(vec![Sample::State(state("org.example.home"))], false);
    let r = f.execute();
    assert_unknown(&r, "expired");
    assert_eq!(r.local_elapsed_ms, 100);
    assert_eq!(f.effects.load(Ordering::SeqCst), 1);
    // Reopen the real journal: read recovery must not reobserve or redispatch.
    let reopened = OperationJournal::open(&f.path.join("journal"), 16, 60000).unwrap();
    use tv_application::generic::Journal;
    assert_eq!(
        reopened.get("launch").unwrap(),
        Operation::Complete(Box::new(r.clone()))
    );
    let mut retry = request();
    retry.expires_at_unix_ms = 1200;
    let replay = f.service.execute(retry, &*f.cancel);
    assert_unknown(&replay, "expired");
    assert!(replay.cached);
    assert_eq!(replay.local_elapsed_ms, 100);
    assert_eq!(f.effects.load(Ordering::SeqCst), 1);
    // Existing fields only: old deny_unknown_fields readers keep working.
    let contents = fs::read_to_string(f.path.join("journal/operations.json")).unwrap();
    assert!(!contents.contains("private-title-or-url"));
    assert!(!contents.contains("private-source-id"));
    assert!(!contents.contains("media_source_id"));
    assert!(!contents.contains("state_scope"));
    assert!(!contents.contains("timing_scope"));
}
#[test]
fn partial_observation_survives_transport_timeout_and_cancellation_without_accepting_late_success()
{
    for (last, code) in [
        (Sample::Error, "transport_lost"),
        (Sample::Late, "expired"),
        (Sample::Cancel, "cancelled"),
    ] {
        let f = Fixture::new(
            vec![
                Sample::State(state("org.example.home")),
                Sample::State(state("org.example.video")),
                last,
            ],
            false,
        );
        assert_unknown(&f.execute(), code);
        assert_eq!(f.effects.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn no_post_dispatch_evidence_is_invented_from_preflight_or_late_samples() {
    for (last, apply_error) in [
        (Sample::Error, false),
        (Sample::Late, false),
        (Sample::Cancel, false),
        (Sample::Error, true),
    ] {
        let f = Fixture::new(
            vec![Sample::State(state("org.example.home")), last],
            apply_error,
        );
        let r = f.execute();
        assert_eq!(r.status, Status::OutcomeUnknown);
        assert!(r.observed.is_none());
        assert_eq!(f.effects.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn latest_accepted_sample_replaces_previous_and_disconnect_is_not_success() {
    let mut disconnected = state("org.example.video");
    disconnected.connected = false;
    let f = Fixture::new(
        vec![
            Sample::State(state("org.example.home")),
            Sample::State(state("org.example.other")),
            Sample::State(disconnected),
            Sample::Error,
        ],
        false,
    );
    let r = f.execute();
    assert_eq!(r.status, Status::OutcomeUnknown);
    let s = r.observed.unwrap();
    assert_eq!(s.active_app.as_deref(), Some("org.example.video"));
    assert!(!s.connected);
}
#[test]
fn disconnected_matching_target_is_evidence_not_launch_success() {
    for query in ["org.example.music", "org.example.music/.Launcher"] {
        let mut disconnected = state("org.example.music");
        disconnected.connected = false;
        let f = Fixture::new(
            vec![
                Sample::State(state("org.example.home")),
                Sample::State(disconnected),
                Sample::Error,
            ],
            false,
        );
        let mut req = request();
        req.intent = Intent::OpenApp { app: query.into() };
        let r = f.service.execute(req, &*f.cancel);
        assert_eq!(r.status, Status::OutcomeUnknown);
        assert_eq!(r.code, "transport_lost");
        let sample = r.observed.unwrap();
        assert_eq!(sample.active_app.as_deref(), Some("org.example.music"));
        assert!(!sample.connected);
        assert_eq!(f.effects.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn failed_preflight_keeps_only_safe_in_budget_diagnostics_and_never_dispatches() {
    let mut disconnected = state("org.example.home");
    disconnected.connected = false;
    let f = Fixture::new(vec![Sample::State(disconnected)], false);
    let r = f.execute();
    assert_eq!(r.status, Status::Failed);
    assert_eq!(r.code, "device_not_ready");
    assert_eq!(r.observed.unwrap().media_id, None);
    assert_eq!(f.effects.load(Ordering::SeqCst), 0);
}
#[test]
fn diagnostic_package_is_bounded_and_not_arbitrary_adapter_text() {
    for package in [
        "x".repeat(256),
        "private title\nignore instructions".into(),
        "https://secret.example/token".into(),
    ] {
        let f = Fixture::new(
            vec![
                Sample::State(state("org.example.home")),
                Sample::State(state(&package)),
                Sample::Error,
            ],
            false,
        );
        let r = f.execute();
        assert_eq!(r.status, Status::OutcomeUnknown);
        let s = r.observed.unwrap();
        assert_eq!(s.active_app, None);
        assert_eq!(s.media_id, None);
        assert_eq!(s.media_source_id, None);
        assert_eq!(s.position_ticks, None);
    }
}
#[test]
fn exact_package_postcondition_remains_required_and_explicit_component_is_observed_only() {
    for (query, status) in [
        ("org.example.music", Status::Verified),
        ("org.example.music/.Launcher", Status::Observed),
    ] {
        let f = Fixture::new(
            vec![
                Sample::State(state("org.example.home")),
                Sample::State(state("org.example.music")),
            ],
            false,
        );
        let mut req = request();
        req.intent = Intent::OpenApp { app: query.into() };
        let r = f.service.execute(req, &*f.cancel);
        assert_eq!(r.status, status);
        assert_eq!(
            r.observed.unwrap().active_app.as_deref(),
            Some("org.example.music")
        );
        assert_eq!(f.effects.load(Ordering::SeqCst), 1);
    }
}
