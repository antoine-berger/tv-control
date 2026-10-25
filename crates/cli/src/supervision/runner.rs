use super::*;
use tv_adapters::runtime::CancelFlag;
use tv_adapters::supervision_store::ContextStore;
use tv_application::{supervision as policy, Cancellation};

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;

fn wait(store: &dyn Store, arm: u64, milliseconds: u64, ctx: &Context<'_>) {
    let store = ContextStore::new(store, ctx);
    let until = ctx.clock.monotonic_ms().saturating_add(milliseconds);
    while ctx.clock.monotonic_ms() < until && ctx.check().is_ok() {
        if !store
            .load()
            .is_ok_and(|record| record.arm == arm && record.mode == Mode::Enabled)
        {
            break;
        }
        ctx.clock
            .sleep_ms(until.saturating_sub(ctx.clock.monotonic_ms()).min(50));
    }
}

pub(super) fn run(configuration: &Path) -> i32 {
    match serve(configuration) {
        Ok(code) => code,
        Err(error) => {
            // SuccessfulExit=false: unsafe/missing state must not become an OS
            // crash loop, even when a blocked record cannot be persisted.
            eprintln!("tv-control supervision: {}", error.code);
            0
        }
    }
}

fn serve(configuration: &Path) -> PortResult<i32> {
    require_platform()?;
    let clock = SystemClock::default();
    let interrupts = Interrupts::install().map_err(Failure::before)?;
    let initial_ctx = Context {
        clock: &clock,
        cancellation: &interrupts,
        deadline_monotonic_ms: clock.monotonic_ms().saturating_add(5000),
    };
    let store = SupervisionStore::open_with_context(&connection(configuration)?, &initial_ctx)?;
    let initial = ContextStore::new(&store, &initial_ctx).load()?;
    // Bootstrap is deliberately disarmed, with no backend or credential access.
    if initial.registration != Registration::Installed || initial.mode != Mode::Enabled {
        return Ok(0);
    }
    restart_on_contention((|| {
        let _lease = store.try_owner_with_context(&initial_ctx)?;
        let mut owner = match policy::enter(&ContextStore::new(&store, &initial_ctx), &initial_ctx)?
        {
            policy::Entry::Owner(owner) => owner,
            _ => return Ok(0),
        };
        let process = process();
        let arm = owner.arm();
        drive_admitted(
            &store,
            &clock,
            &interrupts,
            &mut owner,
            &initial_ctx,
            |ctx| service_runtime::RuntimeBackend::new_with_context(&store, arm, &process, ctx),
        )
    })())
}

fn contention(error: &Failure) -> bool {
    error.code == "supervision_store_busy"
        && error.effect == tv_domain::Effect::None
        && error.status() == tv_domain::Status::Failed
        && !matches!(error.os_error, Some(libc::EPERM | libc::EACCES))
}

// Only an already validated Enabled incarnation reaches this disposition.
// Persistent contention does not authorize replay of any lifecycle action:
// the next OS invocation must re-enter through the durable phase/budget fence.
fn restart_on_contention(result: PortResult<i32>) -> PortResult<i32> {
    match result {
        Err(error) if contention(&error) => Ok(1),
        other => other,
    }
}

/// The caller already owns the lease and the admitted arm. `build` must be
/// read-only: cancellation may require rebuilding under the cleanup context;
/// no failed lifecycle operation or permission/binding refusal is retried.
fn drive_admitted<B: policy::Backend>(
    store: &dyn Store,
    clock: &dyn Clock,
    cancellation: &dyn Cancellation,
    owner: &mut policy::Owner,
    initial: &Context<'_>,
    mut build: impl FnMut(&Context<'_>) -> PortResult<B>,
) -> PortResult<i32> {
    match build(initial) {
        Ok(backend) => drive(store, &backend, clock, cancellation, owner),
        Err(error) if error.status() == tv_domain::Status::Cancelled => {
            let cleanup_cancel = CancelFlag::default();
            let ctx = Context {
                clock,
                cancellation: &cleanup_cancel,
                deadline_monotonic_ms: clock.monotonic_ms().saturating_add(15_000),
            };
            let backend = build(&ctx)?;
            shutdown(store, &backend, owner, &ctx)
        }
        Err(error) => Err(error),
    }
}

/// The caller retains its owner lease throughout admission, this loop and
/// cleanup. No launcher stdin or service-manager API participates in the loop.
fn drive(
    store: &dyn Store,
    backend: &dyn policy::Backend,
    clock: &dyn Clock,
    cancellation: &dyn Cancellation,
    owner: &mut policy::Owner,
) -> PortResult<i32> {
    let should_cleanup = loop {
        if cancellation.is_cancelled() {
            break true;
        }
        let ctx = Context {
            clock,
            cancellation,
            deadline_monotonic_ms: clock.monotonic_ms().saturating_add(60_000),
        };
        match policy::tick(&ContextStore::new(store, &ctx), backend, owner, &ctx) {
            Ok(policy::Step::Wait(ms)) => wait(store, owner.arm(), ms, &ctx),
            Ok(policy::Step::Ready | policy::Step::Degraded) => {
                wait(store, owner.arm(), 1000, &ctx)
            }
            Ok(policy::Step::StopRequested | policy::Step::Interrupted) => break true,
            Err(error) if error.status() == tv_domain::Status::Cancelled => break true,
            Err(error) if contention(&error) => return Ok(1),
            // No extra lifecycle mutation after a permission/binding failure or
            // an uncertain start. Durable Blocked/pending state requires review.
            Ok(policy::Step::Blocked | policy::Step::Conflict) | Err(_) => break false,
        }
    };
    if !should_cleanup {
        return Ok(0);
    }
    let cleanup_cancel = CancelFlag::default();
    let cleanup = Context {
        clock,
        cancellation: &cleanup_cancel,
        deadline_monotonic_ms: clock.monotonic_ms().saturating_add(15_000),
    };
    shutdown(store, backend, owner, &cleanup)
}

fn shutdown(
    store: &dyn Store,
    backend: &dyn policy::Backend,
    owner: &policy::Owner,
    ctx: &Context<'_>,
) -> PortResult<i32> {
    restart_on_contention(
        match policy::shutdown(&ContextStore::new(store, ctx), backend, owner, ctx) {
            Err(error) => Err(error),
            Ok(policy::Exit::Restart) => Ok(1),
            Ok(policy::Exit::Success | policy::Exit::Conflict) => Ok(0),
        },
    )
}
