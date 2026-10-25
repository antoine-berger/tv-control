//! Retry only a pre-transaction busy refusal, under the caller's original context.
use std::time::{Duration, Instant};
use tv_application::{
    supervision::{Record, Store},
    Cancellation, Context,
};
use tv_domain::{Effect, PortResult, Status};

const MAX_CONTENTION: Duration = Duration::from_millis(50);

struct WaitCancellation<'a> {
    outer: &'a dyn Cancellation,
    began: Instant,
}
impl Cancellation for WaitCancellation<'_> {
    fn is_cancelled(&self) -> bool {
        self.outer.is_cancelled() || self.began.elapsed() >= MAX_CONTENTION
    }
}

pub(super) fn retry<T>(
    ctx: &Context<'_>,
    mut operation: impl FnMut() -> PortResult<T>,
) -> PortResult<T> {
    let began = Instant::now();
    loop {
        ctx.check()?;
        let result = operation();
        let error = match result {
            // A successful CAS is committed. Never replace its success with a
            // pre-dispatch deadline/cancellation failure after publication.
            Ok(value) => return Ok(value),
            Err(error)
                if error.code == "supervision_store_busy"
                    && error.effect == Effect::None
                    && error.status() == Status::Failed
                    && !matches!(error.os_error, Some(libc::EPERM | libc::EACCES)) =>
            {
                error
            }
            Err(error) => return Err(error),
        };
        ctx.check()?;
        if began.elapsed() >= MAX_CONTENTION {
            return Err(error);
        }
        // Context::wait also needs the real-time ceiling when an injected clock
        // stalls. Preserve the original clock/deadline and original cancellation;
        // expiry of this additional wait guard is still the original Busy result.
        let cancellation = WaitCancellation {
            outer: ctx.cancellation,
            began,
        };
        let wait = Context {
            clock: ctx.clock,
            cancellation: &cancellation,
            deadline_monotonic_ms: ctx.deadline_monotonic_ms,
        }
        .wait(1);
        if let Err(wait_error) = wait {
            ctx.check()?;
            if began.elapsed() >= MAX_CONTENTION {
                return Err(error);
            }
            return Err(wait_error);
        }
        // The real-time retry allowance is never reset, even with a fake or
        // stalled application clock. The supplied absolute deadline is retained.
        if began.elapsed() >= MAX_CONTENTION {
            ctx.check()?;
            return Err(error);
        }
    }
}

/// A borrowed transaction facade, not a lifecycle retry. Wrapping a fake Store
/// supports composition tests without changing the underlying admission policy.
pub struct ContextStore<'a, 'ctx> {
    store: &'a dyn Store,
    ctx: &'a Context<'ctx>,
}
impl<'a, 'ctx> ContextStore<'a, 'ctx> {
    pub fn new(store: &'a dyn Store, ctx: &'a Context<'ctx>) -> Self {
        Self { store, ctx }
    }
}
impl Store for ContextStore<'_, '_> {
    fn load(&self) -> PortResult<Record> {
        retry(self.ctx, || self.store.load())
    }

    fn compare_and_set(&self, expected_revision: u64, next: &Record) -> PortResult<bool> {
        // `expected_revision` and the target record remain the exact original
        // arguments on every lock acquisition attempt; no read/recalculation.
        retry(self.ctx, || {
            self.store.compare_and_set(expected_revision, next)
        })
    }
}
