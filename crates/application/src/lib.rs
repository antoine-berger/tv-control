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
            return Err(Failure::cancelled("cancelled"));
        }
        if self.clock.monotonic_ms() >= self.deadline_monotonic_ms {
            return Err(Failure::expired("expired"));
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
    fn validate_track(
        &self,
        _device: &str,
        _action: &Action,
        _ctx: &Context<'_>,
    ) -> PortResult<()> {
        Err(Failure::unsupported("track_selection_unsupported"))
    }
    fn prepare(
        &self,
        _device: &str,
        _item_id: &str,
        _source_id: &str,
        _position_ms: u64,
        _ctx: &Context<'_>,
    ) -> PortResult<MediaPlan> {
        Err(Failure::unsupported("unsupported"))
    }
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
        // Resolve relative position once, after the operation acquired device exclusion.
        let action = media_control::resolve(&req.action, &before)?;
        if matches!(
            action,
            Action::SelectAudioTrack { .. } | Action::SelectSubtitleTrack { .. }
        ) {
            media_control::check_identity(&action, &before)?;
            self.catalogue.validate_track(&req.device, &action, ctx)?;
        }
        let plan = match &action {
            Action::ResumeMedia { media } => {
                Some(self.catalogue.resolve(&req.device, media, ctx)?)
            }
            Action::PlayItem {
                item_id,
                source_id,
                position_ms,
            }
            | Action::SeekMedia {
                item_id,
                source_id,
                position_ms,
            } => {
                let mut plan =
                    self.catalogue
                        .prepare(&req.device, item_id, source_id, *position_ms, ctx)?;
                if &plan.item_id != item_id
                    || plan.source_id.as_ref() != Some(source_id)
                    || plan.position_ticks
                        != position_ms.saturating_mul(MEDIA_TICKS_PER_MILLISECOND)
                {
                    return Err(Failure::before("media_plan_identity_mismatch"));
                }
                if matches!(action, Action::SeekMedia { .. })
                    && (before.media_id.as_ref() != Some(item_id)
                        || before.media_source_id.as_ref() != Some(source_id)
                        || !matches!(before.playback, Playback::Paused | Playback::Playing))
                {
                    return Err(Failure::before("seek_media_identity_mismatch"));
                }
                if matches!(req.action, Action::SeekRelative { .. }) {
                    plan.require_paused = true;
                }
                Some(plan)
            }
            _ => None,
        };
        // Catalogue reads can consume the remaining budget or observe cancellation.
        // Even an already-satisfied no-op needs a live request before verification.
        ctx.check()?;
        if !matches!(
            req.action,
            Action::SeekRelative { .. }
                | Action::SelectAudioTrack { .. }
                | Action::SelectSubtitleTrack { .. }
        ) && desired(&action, &before, plan.as_ref(), None)
        {
            return Ok((Status::Verified, before));
        }
        ctx.check()?;
        let dispatch_started = self.clock.monotonic_ms();
        self.devices
            .apply(&req.device, &action, plan.as_ref(), ctx)?;
        let mut previous = None;
        loop {
            ctx.check().map_err(|e| e.after_dispatch())?;
            let observed = self
                .devices
                .observe(&req.device, ctx)
                .map_err(|e| e.after_dispatch())?;
            // A read that completed after expiry must never become a success.
            ctx.check().map_err(|e| e.after_dispatch())?;
            if let Action::SeekRelative { delta_ms, .. } = &req.action {
                if plan.as_ref().is_some_and(|plan| {
                    media_control::relative_verified(
                        *delta_ms,
                        &before,
                        &observed,
                        plan,
                        self.policy.resume_position_tolerance_ms,
                    )
                }) {
                    return Ok((Status::Verified, observed));
                }
                // Keep the immutable absolute target; no retry, extrapolation,
                // delta recalculation, or pause/resume fallback is permitted.
                ctx.wait(self.policy.verification_poll_ms)
                    .map_err(|e| e.after_dispatch())?;
                continue;
            }
            if matches!(
                action,
                Action::Resume | Action::ResumeMedia { .. } | Action::PlayItem { .. }
            ) && observed.playback == Playback::Playing
                && observed.playback_evidence == PlaybackEvidence::PositionProgressRequired
                && observed.position_ticks.is_none()
            {
                return Err(Failure::uncertain("playback_position_unavailable"));
            }
            if let Some(plan) = plan.as_ref() {
                if observed.media_id.as_ref() == Some(&plan.item_id) {
                    if let Some(position) = observed.position_ticks {
                        let elapsed = if matches!(action, Action::SeekMedia { .. })
                            && before.playback == Playback::Paused
                        {
                            0
                        } else {
                            self.clock.monotonic_ms().saturating_sub(dispatch_started)
                        };
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
                            if matches!(action, Action::ResumeMedia { .. }) {
                                return Err(Failure::uncertain("resume_position_not_confirmed"));
                            }
                            previous = Some(observed);
                            ctx.wait(self.policy.verification_poll_ms)
                                .map_err(|e| e.after_dispatch())?;
                            continue;
                        }
                    }
                }
            }
            if matches!(
                action,
                Action::SeekMedia { .. }
                    | Action::SelectAudioTrack { .. }
                    | Action::SelectSubtitleTrack { .. }
            ) && observed.playback != before.playback
            {
                previous = Some(observed);
                ctx.wait(self.policy.verification_poll_ms)
                    .map_err(|e| e.after_dispatch())?;
                continue;
            }
            if desired(&action, &observed, plan.as_ref(), previous.as_ref()) {
                return Ok((Status::Verified, observed));
            }
            previous = Some(observed);
            ctx.wait(self.policy.verification_poll_ms)
                .map_err(|e| e.after_dispatch())?;
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
        if !media_control::valid(&req.action) {
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
            receipt.status = e.status();
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
                receipt.status = e.status();
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
        Action::SelectAudioTrack {
            item_id,
            source_id,
            index,
        } => {
            observed.media_id.as_ref() == Some(item_id)
                && observed.media_source_id.as_ref() == Some(source_id)
                && observed.audio_index == i32::try_from(*index).ok()
                && matches!(observed.playback, Playback::Playing | Playback::Paused)
        }
        Action::SelectSubtitleTrack {
            item_id,
            source_id,
            index,
        } => {
            observed.media_id.as_ref() == Some(item_id)
                && observed.media_source_id.as_ref() == Some(source_id)
                && observed.subtitle_index == Some(*index)
                && matches!(observed.playback, Playback::Playing | Playback::Paused)
        }
        Action::SeekRelative { .. } => false, // Must be resolved exactly once before dispatch.
        Action::SeekMedia { .. } => {
            let Some(plan) = plan else {
                return false;
            };
            observed.media_id.as_ref() == Some(&plan.item_id)
                && observed.media_source_id == plan.source_id
                && matches!(observed.playback, Playback::Playing | Playback::Paused)
                && observed.position_ticks.is_some_and(|p| {
                    if previous.is_none() {
                        p == plan.position_ticks
                    } else {
                        true
                    }
                })
                && (observed.playback == Playback::Paused
                    || previous.is_some_and(|old| {
                        old.media_id == observed.media_id
                            && old.media_source_id == observed.media_source_id
                            && old
                                .position_ticks
                                .zip(observed.position_ticks)
                                .is_some_and(|(a, b)| b > a)
                    }))
        }
        Action::ResumeMedia { .. } | Action::PlayItem { .. } => {
            let Some(plan) = plan else {
                return false;
            };
            let Some(previous) = previous else {
                return false;
            };
            observed.playback == Playback::Playing
                && observed.media_id.as_ref() == Some(&plan.item_id)
                && previous.media_id == observed.media_id
                && (plan.source_id.is_none()
                    || (observed.media_source_id == plan.source_id
                        && previous.media_source_id == plan.source_id))
                && observed.position_ticks.is_some_and(|p| {
                    p >= plan.position_ticks && previous.position_ticks.is_some_and(|old| p > old)
                })
        }
    }
}

pub mod onboarding;

pub mod generic;

pub mod media;

mod media_control;
