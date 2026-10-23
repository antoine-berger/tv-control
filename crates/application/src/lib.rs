//! Use cases and ports. Depends only on the domain and the standard library.
pub mod pipeline;
use tv_domain::*;

pub trait Clock: Send + Sync {
    fn unix_ms(&self) -> u64;
    fn monotonic_ms(&self) -> u64;
    fn sleep_ms(&self, duration: u64);
}
pub trait Cancellation: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

pub struct Context<'a> {
    pub clock: &'a dyn Clock,
    pub cancellation: &'a dyn Cancellation,
    pub deadline_monotonic_ms: u64,
}
impl Context<'_> {
    pub fn check(&self) -> PortResult<()> {
        if self.cancellation.is_cancelled() {
            return Err(Failure::before("cancelled"));
        }
        if self.clock.monotonic_ms() >= self.deadline_monotonic_ms {
            return Err(Failure::before("expired"));
        }
        Ok(())
    }
    pub fn remaining_ms(&self) -> u64 {
        self.deadline_monotonic_ms
            .saturating_sub(self.clock.monotonic_ms())
    }
    pub fn wait(&self, duration: u64) -> PortResult<()> {
        let until = self.clock.monotonic_ms().saturating_add(duration);
        loop {
            self.check()?;
            let left = until.saturating_sub(self.clock.monotonic_ms());
            if left == 0 {
                return Ok(());
            }
            self.clock.sleep_ms(left.min(self.remaining_ms()).min(50));
        }
    }
}

pub trait DeviceControl: Send + Sync {
    fn observe(&self, device: &str, ctx: &Context<'_>) -> PortResult<Observed>;
    /// Adapters must distinguish pre-dispatch failures from uncertain mutation results.
    fn apply(
        &self,
        device: &str,
        action: &Action,
        plan: Option<&MediaPlan>,
        ctx: &Context<'_>,
    ) -> PortResult<()>;
}
pub trait MediaCatalogue: Send + Sync {
    fn resolve(&self, device: &str, alias: &str, ctx: &Context<'_>) -> PortResult<MediaPlan>;
}
pub enum Reservation {
    Acquired,
    Cached(Receipt),
    Busy,
    Conflict,
    Full,
}
pub trait StateStore: Send + Sync {
    fn reserve(&self, request: &Request, now: u64) -> Reservation;
    fn finish(&self, request: &Request, receipt: &Receipt, now: u64);
}
pub trait Control: Send + Sync {
    fn execute(&self, request: Request, cancellation: &dyn Cancellation) -> Receipt;
}

#[derive(Clone, Copy)]
pub struct Policy {
    pub max_budget_ms: u64,
    pub verification_poll_ms: u64,
    pub resume_position_tolerance_ms: u64,
}

pub struct Service<D, M, S, C> {
    pub devices: D,
    pub catalogue: M,
    pub state: S,
    pub clock: C,
    pub policy: Policy,
}
impl<D: DeviceControl, M: MediaCatalogue, S: StateStore, C: Clock> Service<D, M, S, C> {
    fn run(&self, req: &Request, ctx: &Context<'_>) -> PortResult<(Status, Observed)> {
        ctx.check()?;
        let before = self.devices.observe(&req.device, ctx)?;
        ctx.check()?;
        if req.action == Action::Observe {
            return Ok((Status::Observed, before));
        }
        if !before.connected {
            return Err(Failure::before("device_not_ready"));
        }
        let plan = if let Action::ResumeMedia { media } = &req.action {
            Some(self.catalogue.resolve(&req.device, media, ctx)?)
        } else {
            None
        };
        if desired(&req.action, &before, plan.as_ref(), None) {
            return Ok((Status::Verified, before));
        }
        ctx.check()?;
        let dispatch_started = self.clock.monotonic_ms();
        self.devices
            .apply(&req.device, &req.action, plan.as_ref(), ctx)?;
        let mut previous = None;
        loop {
            ctx.check().map_err(|e| Failure::uncertain(e.code))?;
            let observed = self
                .devices
                .observe(&req.device, ctx)
                .map_err(|e| Failure::uncertain(e.code))?;
            // A read that completed after expiry must never become a success.
            ctx.check().map_err(|e| Failure::uncertain(e.code))?;
            if matches!(req.action, Action::Resume | Action::ResumeMedia { .. })
                && observed.playback == Playback::Playing
                && observed.playback_evidence == PlaybackEvidence::PositionProgressRequired
                && observed.position_ticks.is_none()
            {
                return Err(Failure::uncertain("playback_position_unavailable"));
            }
            if let Some(plan) = plan.as_ref() {
                if observed.media_id.as_ref() == Some(&plan.item_id) {
                    if let Some(position) = observed.position_ticks {
                        let elapsed = self.clock.monotonic_ms().saturating_sub(dispatch_started);
                        let tolerance = self
                            .policy
                            .resume_position_tolerance_ms
                            .saturating_mul(MEDIA_TICKS_PER_MILLISECOND);
                        let lower = plan.position_ticks.saturating_sub(tolerance);
                        let upper = plan
                            .position_ticks
                            .saturating_add(elapsed.saturating_mul(MEDIA_TICKS_PER_MILLISECOND))
                            .saturating_add(tolerance);
                        if position < lower || position > upper {
                            return Err(Failure::uncertain("resume_position_not_confirmed"));
                        }
                    }
                }
            }
            if desired(&req.action, &observed, plan.as_ref(), previous.as_ref()) {
                return Ok((Status::Verified, observed));
            }
            previous = Some(observed);
            ctx.wait(self.policy.verification_poll_ms)
                .map_err(|e| Failure::uncertain(e.code))?;
        }
    }
}
impl<D: DeviceControl, M: MediaCatalogue, S: StateStore, C: Clock> Control for Service<D, M, S, C> {
    fn execute(&self, req: Request, cancellation: &dyn Cancellation) -> Receipt {
        let start = self.clock.monotonic_ms();
        let mut receipt = Receipt {
            request_id: req.id.clone(),
            status: Status::Invalid,
            code: "invalid_request",
            observed: None,
            local_elapsed_ms: 0,
            cached: false,
        };
        if !identifier(&req.id) || !identifier(&req.device) {
            return receipt;
        }
        let now = self.clock.unix_ms();
        if req.expires_at_unix_ms <= now {
            receipt.status = Status::Expired;
            receipt.code = "expired";
            return receipt;
        }
        let budget = req.expires_at_unix_ms - now;
        if budget > self.policy.max_budget_ms {
            receipt.code = "budget_exceeds_policy";
            return receipt;
        }
        let ctx = Context {
            clock: &self.clock,
            cancellation,
            deadline_monotonic_ms: start.saturating_add(budget),
        };
        if let Err(e) = ctx.check() {
            receipt.status = Status::Cancelled;
            receipt.code = e.code;
            return receipt;
        }
        match self.state.reserve(&req, start) {
            Reservation::Cached(mut r) => {
                r.cached = true;
                return r;
            }
            Reservation::Busy => {
                receipt.status = Status::Busy;
                receipt.code = "device_busy";
                return receipt;
            }
            Reservation::Conflict => {
                receipt.code = "request_id_conflict";
                return receipt;
            }
            Reservation::Full => {
                receipt.status = Status::Busy;
                receipt.code = "deduplication_capacity";
                return receipt;
            }
            Reservation::Acquired => {}
        }
        match self.run(&req, &ctx) {
            Ok((status, observed)) => {
                receipt.status = status;
                receipt.code = "state_verified";
                receipt.observed = Some(observed);
            }
            Err(e) => {
                receipt.code = e.code;
                receipt.status = if e.effect == Effect::MayHaveChanged {
                    Status::OutcomeUnknown
                } else {
                    match e.code {
                        "cancelled" => Status::Cancelled,
                        "expired" => Status::Expired,
                        "unsupported" => Status::Unsupported,
                        _ => Status::Failed,
                    }
                };
            }
        }
        receipt.local_elapsed_ms = self.clock.monotonic_ms().saturating_sub(start);
        self.state.finish(&req, &receipt, self.clock.monotonic_ms());
        receipt
    }
}

fn desired(
    action: &Action,
    observed: &Observed,
    plan: Option<&MediaPlan>,
    previous: Option<&Observed>,
) -> bool {
    if !observed.connected {
        return false;
    }
    match action {
        Action::Observe => true,
        Action::Pause => observed.playback == Playback::Paused,
        Action::Resume => {
            observed.playback == Playback::Playing
                && match observed.position_ticks {
                    None => observed.playback_evidence == PlaybackEvidence::NativeState,
                    Some(position) => previous.is_some_and(|old| {
                        old.media_id == observed.media_id
                            && old
                                .position_ticks
                                .is_some_and(|old_position| position > old_position)
                    }),
                }
        }
        Action::OpenApp { app } => observed.active_app.as_ref() == Some(app),
        Action::SetVolume { index } => observed.volume_index == Some(*index),
        Action::ResumeMedia { .. } => {
            let Some(plan) = plan else {
                return false;
            };
            let Some(previous) = previous else {
                return false;
            };
            observed.playback == Playback::Playing
                && observed.media_id.as_ref() == Some(&plan.item_id)
                && previous.media_id == observed.media_id
                && observed.position_ticks.is_some_and(|p| {
                    p >= plan.position_ticks && previous.position_ticks.is_some_and(|old| p > old)
                })
        }
    }
}

pub mod onboarding;
