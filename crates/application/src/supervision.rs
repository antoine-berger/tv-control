//! Host lifecycle policy, independent of service managers, files and credentials.
//!
//! The caller holds an exclusive supervisor-owner lease for `enter`, `tick`,
//! `shutdown` and `finish_stop`. Store transactions are short CAS operations;
//! never retain their lock while calling a backend or waiting. A persisted phase
//! is historical evidence, not a process-ownership handle.
use crate::Context;
use tv_domain::{Effect, Failure, PortResult, Status};

pub const MAX_STARTS: u8 = 3;
pub const MAX_UNCLEAN: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Registration {
    Prepared,
    Installed,
    Unknown,
    Removed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Stopped,
    Enabled,
    Blocked,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Monitoring,
    StartInFlight,
    CleanupInFlight,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    Prepared,
    Stopped,
    Armed,
    Monitoring,
    Ready,
    Degraded,
    Starting,
    StartUncertain,
    BackendRefused,
    StartBudgetExhausted,
    UncleanBudgetExhausted,
    InterruptedLifecycle,
    CleanupPending,
    CleanupUnverified,
    CleanupVerified,
}
impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Stopped => "stopped",
            Self::Armed => "armed",
            Self::Monitoring => "monitoring",
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Starting => "starting",
            Self::StartUncertain => "start_uncertain",
            Self::BackendRefused => "backend_refused",
            Self::StartBudgetExhausted => "start_budget_exhausted",
            Self::UncleanBudgetExhausted => "unclean_budget_exhausted",
            Self::InterruptedLifecycle => "interrupted_lifecycle",
            Self::CleanupPending => "cleanup_pending",
            Self::CleanupUnverified => "cleanup_unverified",
            Self::CleanupVerified => "cleanup_verified",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "prepared" => Self::Prepared,
            "stopped" => Self::Stopped,
            "armed" => Self::Armed,
            "monitoring" => Self::Monitoring,
            "ready" => Self::Ready,
            "degraded" => Self::Degraded,
            "starting" => Self::Starting,
            "start_uncertain" => Self::StartUncertain,
            "backend_refused" => Self::BackendRefused,
            "start_budget_exhausted" => Self::StartBudgetExhausted,
            "unclean_budget_exhausted" => Self::UncleanBudgetExhausted,
            "interrupted_lifecycle" => Self::InterruptedLifecycle,
            "cleanup_pending" => Self::CleanupPending,
            "cleanup_unverified" => Self::CleanupUnverified,
            "cleanup_verified" => Self::CleanupVerified,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub revision: u64,
    pub arm: u64,
    pub registration: Registration,
    pub mode: Mode,
    pub phase: Phase,
    pub starts: u8,
    pub unclean: u8,
    pub code: Code,
}
impl Record {
    pub fn prepared() -> Self {
        Self {
            revision: 0,
            arm: 0,
            registration: Registration::Prepared,
            mode: Mode::Stopped,
            phase: Phase::Idle,
            starts: 0,
            unclean: 0,
            code: Code::Prepared,
        }
    }
    pub fn validate(&self) -> PortResult<()> {
        if self.starts > MAX_STARTS
            || self.unclean > MAX_UNCLEAN
            || (self.arm == 0
                && (self.mode != Mode::Stopped || self.starts != 0 || self.unclean != 0))
            || (self.registration != Registration::Installed
                && (self.mode != Mode::Stopped || self.phase != Phase::Idle))
        {
            return Err(Failure::before("supervision_state_invalid"));
        }
        Ok(())
    }
}

/// A CAS must check the expected revision and publish `revision + 1` durably
/// before returning true. Missing/corrupt/unsafe storage is an error, never a
/// fresh Enabled record. False means conflict, without modifying either record.
pub trait Store: Send + Sync {
    fn load(&self) -> PortResult<Record>;
    fn compare_and_set(&self, expected_revision: u64, next: &Record) -> PortResult<bool>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation {
    Absent,
    Ready,
    Degraded,
}
/// Every observation/action revalidates the exact already-approved binding.
/// This port never creates access or receives a TV request. `stop_owned` returns
/// Absent only after fresh owned-runtime cleanup evidence, not a signal receipt.
pub trait Backend: Send + Sync {
    fn observe_owned(&self, ctx: &Context<'_>) -> PortResult<Observation>;
    fn start_owned(&self, ctx: &Context<'_>) -> PortResult<Observation>;
    fn stop_owned(&self, ctx: &Context<'_>) -> PortResult<Observation>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner {
    arm: u64,
    next_start_at: Option<u64>,
}
impl Owner {
    pub fn arm(&self) -> u64 {
        self.arm
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopFence {
    pub arm: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Dormant,
    Blocked,
    Conflict,
    Owner(Owner),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Wait(u64),
    Ready,
    Degraded,
    StopRequested,
    Blocked,
    Interrupted,
    Conflict,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// Enabled after verified cleanup: return nonzero for service-manager retry.
    Restart,
    /// Stopped/Blocked/unsafe: return zero to avoid an OS restart loop.
    Success,
    Conflict,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopResult {
    DisabledOnly,
    Verified,
    Unverified,
    Conflict,
}

/// A short bounded delay is restarted after a process incarnation changes.
/// Counts, not wall clock movement, are the authoritative restart bound.
pub fn backoff_ms(starts: u8) -> u64 {
    match starts {
        0 => 0,
        1 => 1_000,
        2 => 5_000,
        _ => 30_000,
    }
}

fn load(store: &dyn Store, ctx: &Context<'_>) -> PortResult<Record> {
    ctx.check()?;
    let value = store.load()?;
    value.validate()?;
    ctx.check()?;
    Ok(value)
}
fn publish(
    store: &dyn Store,
    current: &Record,
    mut next: Record,
    ctx: &Context<'_>,
) -> PortResult<Option<Record>> {
    ctx.check()?;
    next.revision = current
        .revision
        .checked_add(1)
        .ok_or(Failure::before("supervision_revision_exhausted"))?;
    next.validate()?;
    if store.compare_and_set(current.revision, &next)? {
        Ok(Some(next))
    } else {
        Ok(None)
    }
}

/// The owner lease is already held. Prepared/Stopped/Blocked entries do not
/// touch the backend. Monitoring left by a dead owner consumes durable recovery
/// budget before the new owner can observe or start anything.
pub fn enter(store: &dyn Store, ctx: &Context<'_>) -> PortResult<Entry> {
    let current = load(store, ctx)?;
    if current.registration != Registration::Installed || current.mode == Mode::Stopped {
        return Ok(Entry::Dormant);
    }
    if current.mode == Mode::Blocked {
        return Ok(Entry::Blocked);
    }
    let mut next = current.clone();
    if matches!(current.phase, Phase::StartInFlight | Phase::CleanupInFlight) {
        next.mode = Mode::Blocked;
        next.code = Code::InterruptedLifecycle;
    } else if current.phase == Phase::Monitoring && current.unclean == MAX_UNCLEAN {
        next.mode = Mode::Blocked;
        next.phase = Phase::Idle;
        next.code = Code::UncleanBudgetExhausted;
    } else {
        if current.phase == Phase::Monitoring {
            next.unclean += 1;
        }
        next.phase = Phase::Monitoring;
        next.code = Code::Monitoring;
    }
    let Some(next) = publish(store, &current, next, ctx)? else {
        return Ok(Entry::Conflict);
    };
    if next.mode == Mode::Blocked {
        Ok(Entry::Blocked)
    } else {
        Ok(Entry::Owner(Owner {
            arm: next.arm,
            next_start_at: None,
        }))
    }
}

fn live_step(value: &Record, owner: &Owner) -> Option<Step> {
    if value.arm != owner.arm || value.registration != Registration::Installed {
        Some(Step::Conflict)
    } else if value.mode == Mode::Stopped {
        Some(Step::StopRequested)
    } else if value.mode == Mode::Blocked {
        Some(Step::Blocked)
    } else {
        None
    }
}
fn block(
    store: &dyn Store,
    owner: &Owner,
    code: Code,
    unresolved: bool,
    ctx: &Context<'_>,
) -> PortResult<Step> {
    let current = load(store, ctx)?;
    if let Some(step) = live_step(&current, owner) {
        return Ok(step);
    }
    let mut next = current.clone();
    next.mode = Mode::Blocked;
    if !unresolved {
        next.phase = Phase::Idle;
    }
    next.code = code;
    Ok(if publish(store, &current, next, ctx)?.is_some() {
        Step::Blocked
    } else {
        Step::Conflict
    })
}

/// One bounded iteration. Waiting is delegated to the caller; neither a CAS nor
/// a backend call occurs while the storage transaction remains locked.
pub fn tick(
    store: &dyn Store,
    backend: &dyn Backend,
    owner: &mut Owner,
    ctx: &Context<'_>,
) -> PortResult<Step> {
    let current = match load(store, ctx) {
        Err(error) if error.status() == Status::Cancelled => return Ok(Step::Interrupted),
        result => result?,
    };
    if let Some(step) = live_step(&current, owner) {
        return Ok(step);
    }
    if current.phase != Phase::Monitoring {
        return block(store, owner, Code::InterruptedLifecycle, true, ctx);
    }
    let observation = match backend.observe_owned(ctx) {
        Ok(value) => value,
        Err(error) if error.status() == Status::Cancelled => return Ok(Step::Interrupted),
        Err(_) => return block(store, owner, Code::BackendRefused, false, ctx),
    };
    match ctx.check() {
        Err(error) if error.status() == Status::Cancelled => return Ok(Step::Interrupted),
        result => result?,
    }
    let current = load(store, ctx)?;
    if let Some(step) = live_step(&current, owner) {
        return Ok(step);
    }
    if current.phase != Phase::Monitoring {
        return Ok(Step::Conflict);
    }
    match observation {
        Observation::Ready | Observation::Degraded => {
            owner.next_start_at = None;
            let mut next = current.clone();
            next.code = if observation == Observation::Ready {
                Code::Ready
            } else {
                Code::Degraded
            };
            if current.code != next.code && publish(store, &current, next, ctx)?.is_none() {
                return Ok(Step::Conflict);
            }
            Ok(if observation == Observation::Ready {
                Step::Ready
            } else {
                Step::Degraded
            })
        }
        Observation::Absent => {
            if current.starts == MAX_STARTS {
                return block(store, owner, Code::StartBudgetExhausted, false, ctx);
            }
            let until = *owner.next_start_at.get_or_insert_with(|| {
                ctx.clock
                    .monotonic_ms()
                    .saturating_add(backoff_ms(current.starts))
            });
            let now = ctx.clock.monotonic_ms();
            if now < until {
                return Ok(Step::Wait(until - now));
            }
            let mut next = current.clone();
            next.starts += 1;
            next.phase = Phase::StartInFlight;
            next.code = Code::Starting;
            let Some(admission) = publish(store, &current, next, ctx)? else {
                return Ok(Step::Conflict);
            };
            // Stop may publish while admission is being committed. It wins before
            // dispatch if observed here; a later stop drains this admitted start.
            let fresh = load(store, ctx)?;
            if let Some(step) = live_step(&fresh, owner) {
                return Ok(step);
            }
            if fresh.revision != admission.revision || fresh.phase != Phase::StartInFlight {
                return Ok(Step::Conflict);
            }
            let result = backend.start_owned(ctx);
            owner.next_start_at = None;
            let (after, unresolved) = match result {
                Ok(Observation::Ready) => (Some(Code::Ready), false),
                Ok(Observation::Degraded) => (Some(Code::Degraded), false),
                Ok(Observation::Absent) => (None, true),
                Err(error) if error.status() == Status::Cancelled => return Ok(Step::Interrupted),
                Err(error) => (None, error.effect != Effect::None),
            };
            // Once dispatched, even an expired observation must retain the
            // unresolved admission. It cannot be replayed by the next runner.
            match ctx.check() {
                Err(error) if error.status() == Status::Cancelled => return Ok(Step::Interrupted),
                result => result?,
            }
            let Some(code) = after else {
                return block(
                    store,
                    owner,
                    if unresolved {
                        Code::StartUncertain
                    } else {
                        Code::BackendRefused
                    },
                    unresolved,
                    ctx,
                );
            };
            let current = load(store, ctx)?;
            if let Some(step) = live_step(&current, owner) {
                return Ok(step);
            }
            if current.phase != Phase::StartInFlight {
                return Ok(Step::Conflict);
            }
            let mut next = current.clone();
            next.phase = Phase::Monitoring;
            next.code = code;
            if publish(store, &current, next, ctx)?.is_none() {
                return Ok(Step::Conflict);
            }
            Ok(if code == Code::Ready {
                Step::Ready
            } else {
                Step::Degraded
            })
        }
    }
}

/// Publish the durable stop intent without acquiring/waiting on the owner lease.
/// The caller subsequently drains that owner; this fence is not stop evidence.
pub fn request_stop(store: &dyn Store, ctx: &Context<'_>) -> PortResult<StopFence> {
    for _ in 0..16 {
        let current = load(store, ctx)?;
        if current.mode == Mode::Stopped {
            ctx.check()?;
            return Ok(StopFence { arm: current.arm });
        }
        let mut next = current.clone();
        next.mode = Mode::Stopped;
        next.code = Code::Stopped;
        if publish(store, &current, next, ctx)?.is_some() {
            return Ok(StopFence { arm: current.arm });
        }
    }
    Err(Failure::before("supervision_state_changed"))
}

/// The caller has verified that the old owner/cleanup is resolved and holds the
/// owner lease. This is the only policy operation that resets recovery budgets.
pub fn rearm(store: &dyn Store, ctx: &Context<'_>) -> PortResult<Record> {
    let current = load(store, ctx)?;
    if current.registration != Registration::Installed
        || current.mode == Mode::Enabled
        || current.phase != Phase::Idle
    {
        return Err(Failure::before("supervision_rearm_unresolved"));
    }
    let mut next = current.clone();
    next.arm = current
        .arm
        .checked_add(1)
        .ok_or(Failure::before("supervision_arm_exhausted"))?;
    next.mode = Mode::Enabled;
    next.starts = 0;
    next.unclean = 0;
    next.code = Code::Armed;
    publish(store, &current, next, ctx)?.ok_or(Failure::before("supervision_state_changed"))
}

fn cleanup(
    store: &dyn Store,
    backend: &dyn Backend,
    arm: u64,
    require_stopped: bool,
    ctx: &Context<'_>,
) -> PortResult<StopResult> {
    let current = load(store, ctx)?;
    if current.arm != arm || (require_stopped && current.mode != Mode::Stopped) {
        return Ok(StopResult::Conflict);
    }
    if current.registration != Registration::Installed {
        return Ok(StopResult::DisabledOnly);
    }
    if current.phase == Phase::CleanupInFlight
        || (require_stopped && current.phase == Phase::StartInFlight)
    {
        // A new caller has no authority to replay a lost lifecycle action.
        // Disarming is still durable, but resolution requires external evidence.
        return Ok(StopResult::Unverified);
    }
    let read_only = current.phase == Phase::Idle && current.code == Code::CleanupVerified;
    if !read_only {
        let mut next = current.clone();
        next.phase = Phase::CleanupInFlight;
        next.code = Code::CleanupPending;
        if publish(store, &current, next, ctx)?.is_none() {
            return Ok(StopResult::Conflict);
        }
    }
    let observed = if read_only {
        backend.observe_owned(ctx)
    } else {
        backend.stop_owned(ctx)
    };
    let verified = matches!(observed, Ok(Observation::Absent)) && ctx.check().is_ok();
    // A cleanup outcome outside its budget is unverified. Preserve the pending
    // phase on disk even if a later publication itself is unavailable.
    let current = load(store, ctx)?;
    if current.arm != arm || (require_stopped && current.mode != Mode::Stopped) {
        return Ok(StopResult::Conflict);
    }
    if current.phase
        != if read_only {
            Phase::Idle
        } else {
            Phase::CleanupInFlight
        }
    {
        return Ok(StopResult::Conflict);
    }
    let mut next = current.clone();
    next.code = if verified {
        Code::CleanupVerified
    } else {
        Code::CleanupUnverified
    };
    if verified {
        next.phase = Phase::Idle;
    } else {
        next.phase = Phase::CleanupInFlight;
        if next.mode == Mode::Enabled {
            next.mode = Mode::Blocked;
        }
    }
    if publish(store, &current, next, ctx)?.is_none() {
        return Ok(StopResult::Conflict);
    }
    Ok(if verified {
        StopResult::Verified
    } else {
        StopResult::Unverified
    })
}

/// Explicit stop role, after the old owner has drained and its lease is held.
pub fn finish_stop(
    store: &dyn Store,
    backend: &dyn Backend,
    fence: &StopFence,
    ctx: &Context<'_>,
) -> PortResult<StopResult> {
    cleanup(store, backend, fence.arm, true, ctx)
}

/// Cleanup after a cooperative signal. A fresh, bounded cleanup context is
/// supplied by the caller. This never changes Enabled into Stopped.
pub fn shutdown(
    store: &dyn Store,
    backend: &dyn Backend,
    owner: &Owner,
    ctx: &Context<'_>,
) -> PortResult<Exit> {
    let current = load(store, ctx)?;
    if current.arm != owner.arm {
        return Ok(Exit::Conflict);
    }
    if current.mode == Mode::Blocked {
        return Ok(Exit::Success);
    }
    match cleanup(store, backend, owner.arm, false, ctx)? {
        StopResult::Conflict => Ok(Exit::Conflict),
        StopResult::Verified => {
            let current = load(store, ctx)?;
            if current.arm != owner.arm {
                Ok(Exit::Conflict)
            } else if current.mode == Mode::Enabled {
                Ok(Exit::Restart)
            } else {
                Ok(Exit::Success)
            }
        }
        StopResult::DisabledOnly | StopResult::Unverified => Ok(Exit::Success),
    }
}
