use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tv_adapters::runtime::{CancelFlag, MemoryState};
use tv_application::*;
use tv_domain::*;

#[derive(Clone, Default)]
struct Time(Arc<AtomicU64>);
impl Clock for Time {
    fn unix_ms(&self) -> u64 {
        1_000_000 + self.monotonic_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, n: u64) {
        self.0.fetch_add(n, Ordering::SeqCst);
    }
}
struct Device {
    states: Mutex<VecDeque<Observed>>,
    calls: AtomicUsize,
    failure: Option<Failure>,
    cancel: Option<Arc<CancelFlag>>,
    time: Time,
    read_ms: u64,
}
impl DeviceControl for Device {
    fn observe(&self, _: &str, _: &Context<'_>) -> PortResult<Observed> {
        self.time.sleep_ms(self.read_ms);
        let mut states = self.states.lock().unwrap();
        if states.len() > 1 {
            Ok(states.pop_front().unwrap())
        } else {
            Ok(states.front().unwrap().clone())
        }
    }
    fn apply(&self, _: &str, _: &Action, _: Option<&MediaPlan>, _: &Context<'_>) -> PortResult<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(flag) = &self.cancel {
            flag.cancel();
        }
        self.failure.map_or(Ok(()), Err)
    }
}
struct Catalogue;
impl MediaCatalogue for Catalogue {
    fn resolve(&self, _: &str, _: &str, _: &Context<'_>) -> PortResult<MediaPlan> {
        Ok(MediaPlan {
            item_id: "episode".into(),
            position_ticks: 100,
        })
    }
}
fn state(playback: Playback, position: u64) -> Observed {
    Observed {
        connected: true,
        playback,
        media_id: Some("episode".into()),
        position_ticks: Some(position),
        ..Observed::default()
    }
}
fn service(states: Vec<Observed>) -> Service<Device, Catalogue, MemoryState, Time> {
    let time = Time::default();
    Service {
        devices: Device {
            states: Mutex::new(states.into()),
            calls: AtomicUsize::new(0),
            failure: None,
            cancel: None,
            time: time.clone(),
            read_ms: 0,
        },
        catalogue: Catalogue,
        state: MemoryState::new(16, 10_000),
        clock: time,
        policy: Policy {
            max_budget_ms: 10_000,
            verification_poll_ms: 10,
            resume_position_tolerance_ms: 0,
        },
    }
}
fn request(action: Action) -> Request {
    Request {
        id: "request".into(),
        device: "screen".into(),
        action,
        expires_at_unix_ms: 1_000_100,
    }
}

#[test]
fn verified_pause_and_duplicate_dispatch_once() {
    let s = service(vec![
        state(Playback::Playing, 0),
        state(Playback::Paused, 0),
    ]);
    let req = request(Action::Pause);
    let result = s.execute(req.clone(), &CancelFlag::default());
    assert_eq!(result.status, Status::Verified);
    let repeated = s.execute(req, &CancelFlag::default());
    assert!(repeated.cached);
    assert_eq!(s.devices.calls.load(Ordering::SeqCst), 1);
}
#[test]
fn already_satisfied_pause_does_not_toggle() {
    let s = service(vec![state(Playback::Paused, 0)]);
    assert_eq!(
        s.execute(request(Action::Pause), &CancelFlag::default())
            .status,
        Status::Verified
    );
    assert_eq!(s.devices.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn expired_before_dispatch() {
    let s = service(vec![state(Playback::Playing, 0)]);
    let mut r = request(Action::Pause);
    r.expires_at_unix_ms = 999;
    assert_eq!(s.execute(r, &CancelFlag::default()).status, Status::Expired);
    assert_eq!(s.devices.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn cancellation_before_dispatch() {
    let s = service(vec![state(Playback::Playing, 0)]);
    let flag = CancelFlag::default();
    flag.cancel();
    assert_eq!(
        s.execute(request(Action::Pause), &flag).status,
        Status::Cancelled
    );
    assert_eq!(s.devices.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn cancellation_after_dispatch_is_unknown() {
    let mut s = service(vec![state(Playback::Playing, 0)]);
    let flag = Arc::new(CancelFlag::default());
    s.devices.cancel = Some(flag.clone());
    assert_eq!(
        s.execute(request(Action::Pause), &*flag).status,
        Status::OutcomeUnknown
    );
    assert_eq!(s.devices.calls.load(Ordering::SeqCst), 1);
}
#[test]
fn timeout_after_dispatch_is_unknown_and_not_retried() {
    let s = service(vec![state(Playback::Playing, 0)]);
    let req = request(Action::Pause);
    let result = s.execute(req.clone(), &CancelFlag::default());
    assert_eq!(result.status, Status::OutcomeUnknown);
    assert!(
        s.execute(
            Request {
                expires_at_unix_ms: 1_000_200,
                ..req
            },
            &CancelFlag::default()
        )
        .cached
    );
    assert_eq!(s.devices.calls.load(Ordering::SeqCst), 1);
}
#[test]
fn stale_observation_after_deadline_never_success() {
    let mut s = service(vec![
        state(Playback::Playing, 0),
        state(Playback::Paused, 0),
    ]);
    s.devices.read_ms = 60;
    assert_eq!(
        s.execute(request(Action::Pause), &CancelFlag::default())
            .status,
        Status::OutcomeUnknown
    );
}
#[test]
fn buffering_never_counts_as_playing() {
    let s = service(vec![
        state(Playback::Paused, 0),
        state(Playback::Buffering, 100),
    ]);
    assert_eq!(
        s.execute(request(Action::Resume), &CancelFlag::default())
            .status,
        Status::OutcomeUnknown
    );
}
#[test]
fn media_requires_matching_item_and_position_progress() {
    let s = service(vec![
        state(Playback::Stopped, 0),
        state(Playback::Playing, 100),
        state(Playback::Playing, 110),
    ]);
    assert_eq!(
        s.execute(
            request(Action::ResumeMedia {
                media: "series".into()
            }),
            &CancelFlag::default()
        )
        .status,
        Status::Verified
    );
}
#[test]
fn wrong_media_never_verifies() {
    let mut wrong = state(Playback::Playing, 110);
    wrong.media_id = Some("other_episode".into());
    let s = service(vec![state(Playback::Stopped, 0), wrong]);
    assert_eq!(
        s.execute(
            request(Action::ResumeMedia {
                media: "series".into()
            }),
            &CancelFlag::default()
        )
        .status,
        Status::OutcomeUnknown
    );
}
#[test]
fn no_progress_never_verifies_media() {
    let s = service(vec![
        state(Playback::Stopped, 0),
        state(Playback::Playing, 100),
    ]);
    assert_eq!(
        s.execute(
            request(Action::ResumeMedia {
                media: "series".into()
            }),
            &CancelFlag::default()
        )
        .status,
        Status::OutcomeUnknown
    );
}
#[test]
fn pre_dispatch_error_is_not_unknown() {
    let mut s = service(vec![state(Playback::Playing, 0)]);
    s.devices.failure = Some(Failure::before("unsupported"));
    assert_eq!(
        s.execute(request(Action::Pause), &CancelFlag::default())
            .status,
        Status::Unsupported
    );
}
#[test]
fn ambiguous_dispatch_error_is_unknown() {
    let mut s = service(vec![state(Playback::Playing, 0)]);
    s.devices.failure = Some(Failure::uncertain("transport_lost"));
    assert_eq!(
        s.execute(request(Action::Pause), &CancelFlag::default())
            .status,
        Status::OutcomeUnknown
    );
}
#[test]
fn request_id_cannot_change_action() {
    let s = service(vec![state(Playback::Paused, 0)]);
    s.execute(request(Action::Pause), &CancelFlag::default());
    assert_eq!(
        s.execute(request(Action::Resume), &CancelFlag::default())
            .code,
        "request_id_conflict"
    );
}
#[test]
fn dedup_capacity_refuses_instead_of_evicting_receipt() {
    let state = MemoryState::new(1, 100);
    let req = request(Action::Observe);
    assert!(matches!(state.reserve(&req, 0), Reservation::Acquired));
    let other = Request {
        id: "other".into(),
        device: "other_screen".into(),
        ..req
    };
    assert!(matches!(state.reserve(&other, 1), Reservation::Full));
}
#[test]
fn device_reservation_serializes_different_ids() {
    let state = MemoryState::new(3, 100);
    let req = request(Action::Observe);
    assert!(matches!(state.reserve(&req, 0), Reservation::Acquired));
    assert!(matches!(
        state.reserve(
            &Request {
                id: "second".into(),
                ..req
            },
            0
        ),
        Reservation::Busy
    ));
}
#[test]
fn budget_cannot_be_extended_on_receipt() {
    let s = service(vec![state(Playback::Playing, 0)]);
    let req = Request {
        expires_at_unix_ms: 1_020_000,
        ..request(Action::Pause)
    };
    assert_eq!(
        s.execute(req, &CancelFlag::default()).code,
        "budget_exceeds_policy"
    );
}

#[test]
fn nonpaused_server_flag_without_progress_does_not_verify_resume() {
    let s = service(vec![state(Playback::Playing, 100)]);
    assert_eq!(
        s.execute(request(Action::Resume), &CancelFlag::default())
            .status,
        Status::OutcomeUnknown
    );
}

#[test]
fn contention_time_does_not_reset_global_deadline() {
    struct SlowReservation {
        time: Time,
    }
    impl StateStore for SlowReservation {
        fn reserve(&self, _: &Request, _: u64) -> Reservation {
            self.time.sleep_ms(200);
            Reservation::Acquired
        }
        fn finish(&self, _: &Request, _: &Receipt, _: u64) {}
    }
    let s = service(vec![state(Playback::Playing, 0)]);
    let state = SlowReservation {
        time: s.clock.clone(),
    };
    let s = Service {
        state,
        devices: s.devices,
        catalogue: s.catalogue,
        clock: s.clock,
        policy: s.policy,
    };
    assert_eq!(
        s.execute(request(Action::Pause), &CancelFlag::default())
            .status,
        Status::Expired
    );
    assert_eq!(s.devices.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn concurrent_same_id_reserves_once_then_replays_receipt() {
    let store = Arc::new(MemoryState::new(4, 1000));
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let acquired = Arc::new(AtomicUsize::new(0));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let store = store.clone();
        let barrier = barrier.clone();
        let acquired = acquired.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            if matches!(
                store.reserve(&request(Action::Observe), 0),
                Reservation::Acquired
            ) {
                acquired.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(acquired.load(Ordering::SeqCst), 1);
    let receipt = service(vec![state(Playback::Paused, 0)])
        .execute(request(Action::Observe), &CancelFlag::default());
    store.finish(&request(Action::Observe), &receipt, 1);
    assert!(matches!(
        store.reserve(&request(Action::Observe), 2),
        Reservation::Cached(_)
    ));
}

#[test]
fn missing_position_never_inherits_native_evidence() {
    let mut missing = state(Playback::Playing, 0);
    missing.position_ticks = None;
    missing.playback_evidence = PlaybackEvidence::PositionProgressRequired;
    let s = service(vec![missing]);
    assert_eq!(
        s.execute(request(Action::Resume), &CancelFlag::default())
            .status,
        Status::OutcomeUnknown
    );
    let mut native = state(Playback::Playing, 0);
    native.position_ticks = None;
    native.playback_evidence = PlaybackEvidence::NativeState;
    let s = service(vec![native]);
    assert_eq!(
        s.execute(request(Action::Resume), &CancelFlag::default())
            .status,
        Status::Verified
    );
    assert_eq!(s.devices.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn resume_media_far_ahead_is_not_a_correct_seek() {
    let s = service(vec![
        state(Playback::Stopped, 0),
        state(Playback::Playing, 1_000_000),
        state(Playback::Playing, 1_000_010),
    ]);
    let r = s.execute(
        request(Action::ResumeMedia {
            media: "series".into(),
        }),
        &CancelFlag::default(),
    );
    assert_eq!(r.status, Status::OutcomeUnknown);
    assert_eq!(r.code, "resume_position_not_confirmed");
}

#[test]
fn resume_media_tolerance_accounts_for_elapsed_time() {
    let mut s = service(vec![
        state(Playback::Stopped, 0),
        state(Playback::Playing, 100_000),
        state(Playback::Playing, 200_000),
    ]);
    s.devices.read_ms = 10;
    assert_eq!(
        s.execute(
            request(Action::ResumeMedia {
                media: "series".into()
            }),
            &CancelFlag::default()
        )
        .status,
        Status::Verified
    );
}

#[test]
fn configured_wait_is_complete_cancellable_and_deadline_bounded() {
    let time = Time::default();
    let flag = CancelFlag::default();
    let ctx = Context {
        clock: &time,
        cancellation: &flag,
        deadline_monotonic_ms: 1000,
    };
    ctx.wait(500).unwrap();
    assert_eq!(time.monotonic_ms(), 500);
    struct CancelAt(Time);
    impl Cancellation for CancelAt {
        fn is_cancelled(&self) -> bool {
            self.0.monotonic_ms() >= 625
        }
    }
    let cancel = CancelAt(time.clone());
    let ctx = Context {
        clock: &time,
        cancellation: &cancel,
        deadline_monotonic_ms: 1000,
    };
    assert_eq!(ctx.wait(500).unwrap_err().code, "cancelled");
    assert_eq!(time.monotonic_ms(), 650);
    let ctx = Context {
        clock: &time,
        cancellation: &flag,
        deadline_monotonic_ms: 700,
    };
    assert_eq!(ctx.wait(500).unwrap_err().code, "expired");
    assert_eq!(time.monotonic_ms(), 700);
}

#[test]
fn aliases_sharing_a_physical_group_reserve_once() {
    let store = Arc::new(
        MemoryState::new(4, 1000).with_resource_groups(
            [
                ("first".into(), "physical_screen".into()),
                ("second".into(), "physical_screen".into()),
            ]
            .into_iter()
            .collect(),
        ),
    );
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let acquired = Arc::new(AtomicUsize::new(0));
    let mut workers = Vec::new();
    for alias in ["first", "second"] {
        let store = store.clone();
        let barrier = barrier.clone();
        let acquired = acquired.clone();
        workers.push(std::thread::spawn(move || {
            let req = Request {
                id: alias.into(),
                device: alias.into(),
                ..request(Action::Observe)
            };
            barrier.wait();
            if matches!(store.reserve(&req, 0), Reservation::Acquired) {
                acquired.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(acquired.load(Ordering::SeqCst), 1);
}
