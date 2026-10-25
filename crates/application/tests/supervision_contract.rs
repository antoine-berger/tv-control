//! Pure lifecycle policy: fake clocks/ports, no process or device access.
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Mutex,
};
use tv_application::{supervision::*, Cancellation, Clock, Context};
use tv_domain::{Failure, PortResult};

#[derive(Default)]
struct Time {
    mono: AtomicU64,
    wall: AtomicU64,
}
impl Clock for Time {
    fn unix_ms(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
    fn monotonic_ms(&self) -> u64 {
        self.mono.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, value: u64) {
        self.mono.fetch_add(value, Ordering::SeqCst);
    }
}
#[derive(Default)]
struct Cancel(AtomicBool);
impl Cancellation for Cancel {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
fn context<'a>(clock: &'a Time, cancel: &'a Cancel) -> Context<'a> {
    Context {
        clock,
        cancellation: cancel,
        deadline_monotonic_ms: clock.monotonic_ms() + 100_000,
    }
}
struct Memory {
    record: Mutex<Record>,
    fail_cas: AtomicBool,
    conflict_cas: AtomicBool,
    stop_after_admission: AtomicBool,
    writes: AtomicUsize,
}
impl Memory {
    fn new() -> Self {
        let mut record = Record::prepared();
        record.registration = Registration::Installed;
        Self {
            record: Mutex::new(record),
            fail_cas: AtomicBool::new(false),
            conflict_cas: AtomicBool::new(false),
            stop_after_admission: AtomicBool::new(false),
            writes: AtomicUsize::new(0),
        }
    }
}
impl Store for Memory {
    fn load(&self) -> PortResult<Record> {
        Ok(self.record.lock().unwrap().clone())
    }
    fn compare_and_set(&self, expected: u64, next: &Record) -> PortResult<bool> {
        if self.fail_cas.swap(false, Ordering::SeqCst) {
            return Err(Failure::before("injected_storage_error"));
        }
        if self.conflict_cas.swap(false, Ordering::SeqCst) {
            return Ok(false);
        }
        let mut current = self.record.lock().unwrap();
        if current.revision != expected {
            return Ok(false);
        }
        assert_eq!(next.revision, expected + 1);
        next.validate()?;
        *current = next.clone();
        self.writes.fetch_add(1, Ordering::SeqCst);
        if next.phase == Phase::StartInFlight
            && self.stop_after_admission.swap(false, Ordering::SeqCst)
        {
            current.mode = Mode::Stopped;
            current.revision += 1;
        }
        Ok(true)
    }
}
struct Fake<'a> {
    store: &'a Memory,
    clock: &'a Time,
    observe: Mutex<PortResult<Observation>>,
    start: Mutex<PortResult<Observation>>,
    stop: Mutex<PortResult<Observation>>,
    reads: AtomicUsize,
    starts: AtomicUsize,
    stops: AtomicUsize,
    stop_during_start: AtomicBool,
    expire_after_start: AtomicBool,
    expire_after_stop: AtomicBool,
    expire_after_observe: AtomicBool,
    conflict_after_start: AtomicBool,
    cancel_during_start: Option<&'a Cancel>,
}
impl<'a> Fake<'a> {
    fn new(store: &'a Memory, clock: &'a Time) -> Self {
        Self {
            store,
            clock,
            observe: Mutex::new(Ok(Observation::Absent)),
            start: Mutex::new(Ok(Observation::Ready)),
            stop: Mutex::new(Ok(Observation::Absent)),
            reads: AtomicUsize::new(0),
            starts: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
            stop_during_start: AtomicBool::new(false),
            expire_after_start: AtomicBool::new(false),
            expire_after_stop: AtomicBool::new(false),
            expire_after_observe: AtomicBool::new(false),
            conflict_after_start: AtomicBool::new(false),
            cancel_during_start: None,
        }
    }
    fn calls(&self) -> (usize, usize, usize) {
        (
            self.reads.load(Ordering::SeqCst),
            self.starts.load(Ordering::SeqCst),
            self.stops.load(Ordering::SeqCst),
        )
    }
}
impl Backend for Fake<'_> {
    fn observe_owned(&self, ctx: &Context<'_>) -> PortResult<Observation> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.expire_after_observe.load(Ordering::SeqCst) {
            self.clock
                .mono
                .store(ctx.deadline_monotonic_ms, Ordering::SeqCst);
        }
        *self.observe.lock().unwrap()
    }
    fn start_owned(&self, ctx: &Context<'_>) -> PortResult<Observation> {
        // Admission and counter must already be durable before the port sees us.
        let state = self.store.load().unwrap();
        assert_eq!(state.phase, Phase::StartInFlight);
        assert_eq!(
            state.starts as usize,
            self.starts.load(Ordering::SeqCst) + 1
        );
        self.starts.fetch_add(1, Ordering::SeqCst);
        if self.conflict_after_start.load(Ordering::SeqCst) {
            self.store.conflict_cas.store(true, Ordering::SeqCst);
        }
        if let Some(cancel) = self.cancel_during_start {
            cancel.0.store(true, Ordering::SeqCst);
        }
        if self.stop_during_start.load(Ordering::SeqCst) {
            request_stop(self.store, ctx).unwrap();
        }
        if self.expire_after_start.load(Ordering::SeqCst) {
            self.clock
                .mono
                .store(ctx.deadline_monotonic_ms, Ordering::SeqCst);
        }
        *self.start.lock().unwrap()
    }
    fn stop_owned(&self, ctx: &Context<'_>) -> PortResult<Observation> {
        assert_eq!(self.store.load().unwrap().phase, Phase::CleanupInFlight);
        self.stops.fetch_add(1, Ordering::SeqCst);
        if self.expire_after_stop.load(Ordering::SeqCst) {
            self.clock
                .mono
                .store(ctx.deadline_monotonic_ms, Ordering::SeqCst);
        }
        *self.stop.lock().unwrap()
    }
}
fn arm(store: &Memory, ctx: &Context<'_>) -> Owner {
    rearm(store, ctx).unwrap();
    let Entry::Owner(owner) = enter(store, ctx).unwrap() else {
        panic!("owner admission expected")
    };
    owner
}

#[test]
fn disarmed_and_uninstalled_entry_never_touches_backend() {
    let clock = Time::default();
    let cancel = Cancel::default();
    let ctx = context(&clock, &cancel);
    for registration in [
        Registration::Prepared,
        Registration::Installed,
        Registration::Unknown,
        Registration::Removed,
    ] {
        let store = Memory::new();
        store.record.lock().unwrap().registration = registration;
        let backend = Fake::new(&store, &clock);
        assert_eq!(enter(&store, &ctx).unwrap(), Entry::Dormant);
        let fence = request_stop(&store, &ctx).unwrap();
        if registration != Registration::Installed {
            assert_eq!(
                finish_stop(&store, &backend, &fence, &ctx).unwrap(),
                StopResult::DisabledOnly
            );
        }
        assert_eq!(backend.calls(), (0, 0, 0));
        assert_eq!(store.writes.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn runtime_start_budget_survives_fresh_owners_readiness_and_wall_clock_changes() {
    let store = Memory::new();
    let clock = Time::default();
    let cancel = Cancel::default();
    let backend = Fake::new(&store, &clock);
    let mut owner = arm(&store, &context(&clock, &cancel));
    for count in 1..=MAX_STARTS {
        loop {
            match tick(&store, &backend, &mut owner, &context(&clock, &cancel)).unwrap() {
                Step::Wait(ms) => clock.sleep_ms(ms),
                Step::Ready => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(store.load().unwrap().starts, count);
        *backend.observe.lock().unwrap() = Ok(Observation::Ready);
        assert_eq!(
            tick(&store, &backend, &mut owner, &context(&clock, &cancel)).unwrap(),
            Step::Ready
        );
        assert_eq!(store.load().unwrap().starts, count);
        clock
            .wall
            .store(if count % 2 == 0 { 0 } else { u64::MAX }, Ordering::SeqCst);
        // New runner incarnation sees the durable history, not a fresh budget.
        let Entry::Owner(next) = enter(&store, &context(&clock, &cancel)).unwrap() else {
            panic!()
        };
        owner = next;
        *backend.observe.lock().unwrap() = Ok(Observation::Absent);
    }
    assert_eq!(
        tick(&store, &backend, &mut owner, &context(&clock, &cancel)).unwrap(),
        Step::Blocked
    );
    assert_eq!(backend.starts.load(Ordering::SeqCst), 3);
    assert_eq!(store.load().unwrap().code, Code::StartBudgetExhausted);
    assert_eq!(
        enter(&store, &context(&clock, &cancel)).unwrap(),
        Entry::Blocked
    );
}

#[test]
fn unclean_incarnation_budget_is_charged_before_any_backend_call() {
    let store = Memory::new();
    let clock = Time::default();
    let cancel = Cancel::default();
    let ctx = context(&clock, &cancel);
    let backend = Fake::new(&store, &clock);
    arm(&store, &ctx);
    for value in 1..=MAX_UNCLEAN {
        assert!(matches!(enter(&store, &ctx).unwrap(), Entry::Owner(_)));
        assert_eq!(store.load().unwrap().unclean, value);
    }
    assert_eq!(enter(&store, &ctx).unwrap(), Entry::Blocked);
    assert_eq!(store.load().unwrap().code, Code::UncleanBudgetExhausted);
    assert_eq!(backend.calls(), (0, 0, 0));
}

#[test]
fn crash_during_start_or_cleanup_blocks_without_replaying() {
    for phase in [Phase::StartInFlight, Phase::CleanupInFlight] {
        let store = Memory::new();
        let clock = Time::default();
        let cancel = Cancel::default();
        let ctx = context(&clock, &cancel);
        arm(&store, &ctx);
        store.record.lock().unwrap().phase = phase;
        let backend = Fake::new(&store, &clock);
        assert_eq!(enter(&store, &ctx).unwrap(), Entry::Blocked);
        assert_eq!(store.load().unwrap().phase, phase);
        assert!(rearm(&store, &ctx).is_err());
        assert_eq!(backend.calls(), (0, 0, 0));
    }
}

#[test]
fn uncertain_start_is_persistently_blocked_after_exactly_one_call() {
    let store = Memory::new();
    let clock = Time::default();
    let cancel = Cancel::default();
    let ctx = context(&clock, &cancel);
    let mut owner = arm(&store, &ctx);
    let backend = Fake::new(&store, &clock);
    *backend.start.lock().unwrap() = Err(Failure::uncertain("injected_lost_ack"));
    assert_eq!(
        tick(&store, &backend, &mut owner, &ctx).unwrap(),
        Step::Blocked
    );
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
    let state = store.load().unwrap();
    assert_eq!(
        (state.mode, state.phase, state.starts),
        (Mode::Blocked, Phase::StartInFlight, 1)
    );
    assert_eq!(enter(&store, &ctx).unwrap(), Entry::Blocked);
    assert!(rearm(&store, &ctx).is_err());
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
}

#[test]
fn known_predispatch_start_refusal_blocks_without_fabricating_uncertainty() {
    let store = Memory::new();
    let clock = Time::default();
    let cancel = Cancel::default();
    let ctx = context(&clock, &cancel);
    let mut owner = arm(&store, &ctx);
    let backend = Fake::new(&store, &clock);
    *backend.start.lock().unwrap() = Err(Failure::before("injected_start_refused"));
    assert_eq!(
        tick(&store, &backend, &mut owner, &ctx).unwrap(),
        Step::Blocked
    );
    let state = store.load().unwrap();
    assert_eq!(
        (state.mode, state.phase, state.starts, state.code),
        (Mode::Blocked, Phase::Idle, 1, Code::BackendRefused)
    );
    assert_eq!(enter(&store, &ctx).unwrap(), Entry::Blocked);
    assert_eq!(
        shutdown(&store, &backend, &owner, &ctx).unwrap(),
        Exit::Success
    );
    assert_eq!(backend.calls(), (1, 1, 0));
}

#[test]
fn persistence_failure_and_stop_after_admission_prevent_dispatch() {
    for fail in [true, false] {
        let store = Memory::new();
        let clock = Time::default();
        let cancel = Cancel::default();
        let ctx = context(&clock, &cancel);
        let mut owner = arm(&store, &ctx);
        let backend = Fake::new(&store, &clock);
        if fail {
            store.fail_cas.store(true, Ordering::SeqCst);
        } else {
            store.stop_after_admission.store(true, Ordering::SeqCst);
        }
        let result = tick(&store, &backend, &mut owner, &ctx);
        if fail {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap(), Step::StopRequested);
        }
        assert_eq!(backend.starts.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn stop_during_start_is_drained_before_verification_and_never_reenabled() {
    let store = Memory::new();
    let clock = Time::default();
    let cancel = Cancel::default();
    let ctx = context(&clock, &cancel);
    let mut owner = arm(&store, &ctx);
    let backend = Fake::new(&store, &clock);
    backend.stop_during_start.store(true, Ordering::SeqCst);
    assert_eq!(
        tick(&store, &backend, &mut owner, &ctx).unwrap(),
        Step::StopRequested
    );
    assert_eq!(store.load().unwrap().phase, Phase::StartInFlight);
    assert!(rearm(&store, &ctx).is_err());
    assert_eq!(
        shutdown(&store, &backend, &owner, &ctx).unwrap(),
        Exit::Success
    );
    let state = store.load().unwrap();
    assert_eq!((state.mode, state.phase), (Mode::Stopped, Phase::Idle));
    assert_eq!(backend.calls(), (1, 1, 1));
}

#[test]
fn cooperative_signal_cleanup_preserves_enabled_but_requests_os_restart() {
    let store = Memory::new();
    let clock = Time::default();
    let cancel = Cancel::default();
    let ctx = context(&clock, &cancel);
    let owner = arm(&store, &ctx);
    let backend = Fake::new(&store, &clock);
    assert_eq!(
        shutdown(&store, &backend, &owner, &ctx).unwrap(),
        Exit::Restart
    );
    let state = store.load().unwrap();
    assert_eq!(
        (state.mode, state.phase, state.unclean),
        (Mode::Enabled, Phase::Idle, 0)
    );
    assert_eq!(backend.calls(), (0, 0, 1));
}

#[test]
fn live_degraded_and_refused_observations_never_trigger_blind_restarts() {
    for observed in [
        Ok(Observation::Degraded),
        Err(Failure::before("permission_denied")),
    ] {
        let store = Memory::new();
        let clock = Time::default();
        let cancel = Cancel::default();
        let ctx = context(&clock, &cancel);
        let mut owner = arm(&store, &ctx);
        let backend = Fake::new(&store, &clock);
        *backend.observe.lock().unwrap() = observed;
        let step = tick(&store, &backend, &mut owner, &ctx).unwrap();
        assert!(matches!(step, Step::Degraded | Step::Blocked));
        assert_eq!(backend.calls(), (1, 0, 0));
        if step == Step::Blocked {
            assert_eq!(
                shutdown(&store, &backend, &owner, &ctx).unwrap(),
                Exit::Success
            );
            assert_eq!(backend.calls(), (1, 0, 0));
        }
    }
}

#[test]
fn late_start_or_cleanup_cannot_publish_success() {
    for during_start in [true, false] {
        let store = Memory::new();
        let clock = Time::default();
        let cancel = Cancel::default();
        let ctx = context(&clock, &cancel);
        let mut owner = arm(&store, &ctx);
        let backend = Fake::new(&store, &clock);
        if during_start {
            backend.expire_after_start.store(true, Ordering::SeqCst);
            assert!(tick(&store, &backend, &mut owner, &ctx).is_err());
            assert_eq!(store.load().unwrap().phase, Phase::StartInFlight);
        } else {
            backend.expire_after_stop.store(true, Ordering::SeqCst);
            assert!(shutdown(&store, &backend, &owner, &ctx).is_err());
            assert_eq!(store.load().unwrap().phase, Phase::CleanupInFlight);
        }
        assert_eq!(
            enter(&store, &context(&clock, &cancel)).unwrap(),
            Entry::Blocked
        );
    }
}

#[test]
fn old_stop_and_old_owner_cannot_act_on_a_new_arm() {
    let store = Memory::new();
    let clock = Time::default();
    let cancel = Cancel::default();
    let ctx = context(&clock, &cancel);
    let mut owner = arm(&store, &ctx);
    let backend = Fake::new(&store, &clock);
    let stop = request_stop(&store, &ctx).unwrap();
    assert_eq!(
        finish_stop(&store, &backend, &stop, &ctx).unwrap(),
        StopResult::Verified
    );
    rearm(&store, &ctx).unwrap();
    let calls = backend.calls();
    assert_eq!(
        finish_stop(&store, &backend, &stop, &ctx).unwrap(),
        StopResult::Conflict
    );
    assert_eq!(
        shutdown(&store, &backend, &owner, &ctx).unwrap(),
        Exit::Conflict
    );
    assert_eq!(
        tick(&store, &backend, &mut owner, &ctx).unwrap(),
        Step::Conflict
    );
    assert_eq!(backend.calls(), calls);
}

#[test]
fn failed_cleanup_remains_unverified_and_cannot_rearm() {
    let store = Memory::new();
    let clock = Time::default();
    let cancel = Cancel::default();
    let ctx = context(&clock, &cancel);
    arm(&store, &ctx);
    let backend = Fake::new(&store, &clock);
    *backend.stop.lock().unwrap() = Err(Failure::uncertain("injected_cleanup_lost"));
    let stop = request_stop(&store, &ctx).unwrap();
    assert_eq!(
        finish_stop(&store, &backend, &stop, &ctx).unwrap(),
        StopResult::Unverified
    );
    let state = store.load().unwrap();
    assert_eq!(
        (state.mode, state.phase),
        (Mode::Stopped, Phase::CleanupInFlight)
    );
    assert!(rearm(&store, &ctx).is_err());
    let calls = backend.calls();
    assert_eq!(
        finish_stop(&store, &backend, &stop, &ctx).unwrap(),
        StopResult::Unverified
    );
    assert_eq!(
        backend.calls(),
        calls,
        "unknown cleanup must not be replayed"
    );
}

#[test]
fn completed_stop_fence_verifies_with_a_read_without_repeating_the_write() {
    let store = Memory::new();
    let clock = Time::default();
    let cancel = Cancel::default();
    let ctx = context(&clock, &cancel);
    arm(&store, &ctx);
    let backend = Fake::new(&store, &clock);
    let stop = request_stop(&store, &ctx).unwrap();
    assert_eq!(
        finish_stop(&store, &backend, &stop, &ctx).unwrap(),
        StopResult::Verified
    );
    assert_eq!(
        finish_stop(&store, &backend, &stop, &ctx).unwrap(),
        StopResult::Verified
    );
    assert_eq!(backend.calls(), (1, 0, 1));
}

#[test]
fn acknowledgement_publication_conflict_cannot_replay_the_start() {
    let store = Memory::new();
    let clock = Time::default();
    let cancel = Cancel::default();
    let ctx = context(&clock, &cancel);
    let mut owner = arm(&store, &ctx);
    let backend = Fake::new(&store, &clock);
    backend.conflict_after_start.store(true, Ordering::SeqCst);
    assert_eq!(
        tick(&store, &backend, &mut owner, &ctx).unwrap(),
        Step::Conflict
    );
    assert_eq!(store.load().unwrap().phase, Phase::StartInFlight);
    assert_eq!(
        tick(&store, &backend, &mut owner, &ctx).unwrap(),
        Step::Blocked
    );
    assert_eq!(enter(&store, &ctx).unwrap(), Entry::Blocked);
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
}

#[test]
fn cancelled_admitted_start_uses_fresh_cleanup_context_without_disarming() {
    let store = Memory::new();
    let clock = Time::default();
    let cancel = Cancel::default();
    let ctx = context(&clock, &cancel);
    let mut owner = arm(&store, &ctx);
    let mut backend = Fake::new(&store, &clock);
    backend.cancel_during_start = Some(&cancel);
    assert_eq!(
        tick(&store, &backend, &mut owner, &ctx).unwrap(),
        Step::Interrupted
    );
    assert_eq!(store.load().unwrap().phase, Phase::StartInFlight);
    let cleanup_cancel = Cancel::default();
    assert_eq!(
        shutdown(&store, &backend, &owner, &context(&clock, &cleanup_cancel)).unwrap(),
        Exit::Restart
    );
    let state = store.load().unwrap();
    assert_eq!(
        (state.mode, state.phase, state.starts),
        (Mode::Enabled, Phase::Idle, 1)
    );
    assert_eq!(backend.calls(), (1, 1, 1));
}

#[test]
fn cancellation_before_tick_and_expired_observation_cannot_start_or_claim_ready() {
    for cancelled in [false, true] {
        let store = Memory::new();
        let clock = Time::default();
        let cancel = Cancel::default();
        let ctx = context(&clock, &cancel);
        let mut owner = arm(&store, &ctx);
        let backend = Fake::new(&store, &clock);
        *backend.observe.lock().unwrap() = Ok(Observation::Ready);
        if cancelled {
            cancel.0.store(true, Ordering::SeqCst);
        } else {
            backend.expire_after_observe.store(true, Ordering::SeqCst);
        }
        let result = tick(&store, &backend, &mut owner, &ctx);
        if cancelled {
            assert_eq!(result.unwrap(), Step::Interrupted);
        } else {
            assert!(result.is_err());
        }
        assert_eq!(backend.starts.load(Ordering::SeqCst), 0);
        assert_eq!(backend.stops.load(Ordering::SeqCst), 0);
        assert_ne!(store.load().unwrap().code, Code::Ready);
    }
}

#[test]
fn state_revision_and_arm_overflow_fail_before_backend_or_publication() {
    for arm_overflow in [false, true] {
        let store = Memory::new();
        let clock = Time::default();
        let cancel = Cancel::default();
        if arm_overflow {
            store.record.lock().unwrap().arm = u64::MAX;
        } else {
            store.record.lock().unwrap().revision = u64::MAX;
        }
        let before = store.load().unwrap();
        assert!(rearm(&store, &context(&clock, &cancel)).is_err());
        assert_eq!(store.load().unwrap(), before);
        assert_eq!(store.writes.load(Ordering::SeqCst), 0);
    }
}
