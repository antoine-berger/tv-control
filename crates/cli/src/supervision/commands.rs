use super::*;
use tv_adapters::service_platform::Launchd;
use tv_application::supervision::{self as policy, StopResult};

fn open(configuration: &Path, ctx: &Context<'_>) -> PortResult<SupervisionStore> {
    SupervisionStore::open_with_context(&connection(configuration)?, ctx)
}
fn owner_after_drain(
    store: &SupervisionStore,
    ctx: &Context<'_>,
) -> PortResult<tv_adapters::supervision_store::OwnerLease> {
    loop {
        ctx.check()?;
        match store.try_owner() {
            Ok(owner) => return Ok(owner),
            Err(error)
                if matches!(
                    error.code,
                    "supervision_owner_busy" | "supervision_store_busy"
                ) && error.effect == tv_domain::Effect::None =>
            {
                ctx.wait(20)?
            }
            Err(error) => return Err(error),
        }
    }
}

pub(super) fn stop_store(
    store: &SupervisionStore,
    ctx: &Context<'_>,
) -> PortResult<policy::StopFence> {
    let fence = policy::request_stop(&store.with_context(ctx), ctx)?;
    let _owner = owner_after_drain(store, ctx)?;
    let current = store.load_with_context(ctx)?;
    if current.arm != fence.arm || current.mode != Mode::Stopped {
        return Err(Failure::before("supervision_stop_superseded"));
    }
    if current.registration != Registration::Installed {
        return Err(Failure::uncertain(
            "service_registration_cleanup_unverified",
        ));
    }
    let process = process();
    let backend =
        service_runtime::RuntimeBackend::new_with_context(store, fence.arm, &process, ctx)?;
    match policy::finish_stop(&store.with_context(ctx), &backend, &fence, ctx)? {
        StopResult::Verified => Ok(fence),
        StopResult::DisabledOnly => Err(Failure::uncertain(
            "service_registration_cleanup_unverified",
        )),
        StopResult::Unverified => Err(Failure::uncertain("supervision_cleanup_unverified")),
        StopResult::Conflict => Err(Failure::before("supervision_stop_superseded")),
    }
}

pub(super) fn install(configuration: &Path, ctx: &Context<'_>) -> PortResult<i32> {
    require_platform()?;
    let binding = service_runtime::plan_binding(configuration, ctx)?;
    ctx.check()?;
    let lock = installation_lock::InstallationLock::acquire(&binding.launcher)?;
    // Capturing the plan may race with the ordinary installer. Validate all pins
    // again while holding its exact publication lock, through OS admission.
    service_runtime::check_binding(&binding, &process(), ctx)?;
    let store = SupervisionStore::prepare(&connection(configuration)?, &binding, true)?;
    let result = bootstrap_prepared(&store, &process(), ctx);
    lock.release()?;
    let code = result?;
    println!(
        "{}",
        json!({"status":"installed_disarmed","activated":false,"label":binding.label,
        "next_step":"Explicit service resume --approve-persistence is required."})
    );
    Ok(code)
}

pub(super) fn bootstrap_prepared(
    store: &SupervisionStore,
    process: &dyn tv_adapters::process::Process,
    ctx: &Context<'_>,
) -> PortResult<i32> {
    let binding = store.binding_with_context(ctx)?;
    let service = spec(&binding);
    let bytes = service.render()?;
    let _owner = store.try_owner_with_context(ctx)?;
    let prepared = store.load_with_context(ctx)?;
    if prepared.registration != Registration::Prepared
        || prepared.mode != Mode::Stopped
        || prepared.phase != Phase::Idle
    {
        return Err(Failure::before("supervision_bootstrap_not_prepared"));
    }
    // The durable Prepared/Stopped record precedes file creation and bootstrap.
    ctx.check()?;
    registration_marker::create(&binding)?;
    artifact::create(&binding.artifact, &bytes)?;
    ctx.check()?;
    let platform = Launchd { process };
    let outcome = (|| -> PortResult<i32> {
        platform.bootstrap(&service, ctx)?;
        ctx.check().map_err(Failure::after_dispatch)?;
        publish_registration(
            store,
            &prepared,
            Registration::Installed,
            Code::Stopped,
            ctx,
        )
        .map_err(Failure::after_dispatch)?;
        Ok(0)
    })();
    if outcome.is_err() {
        // This state-only cleanup remains possible after cancellation/expiry.
        // CAS of the original revision preserves a concurrent Stop. No OS retry.
        let clock = SystemClock::default();
        let cancel = tv_adapters::runtime::CancelFlag::default();
        let cleanup = Context {
            clock: &clock,
            cancellation: &cancel,
            deadline_monotonic_ms: 100,
        };
        let _ = publish_registration(
            store,
            &prepared,
            Registration::Unknown,
            Code::BackendRefused,
            &cleanup,
        );
    }
    outcome
}

fn block_arm(store: &SupervisionStore, arm: u64) {
    // Best-effort safety publication also runs after the original operation was
    // cancelled. This short cleanup context authorizes no OS/backend action.
    let clock = SystemClock::default();
    let cancel = tv_adapters::runtime::CancelFlag::default();
    let ctx = Context {
        clock: &clock,
        cancellation: &cancel,
        deadline_monotonic_ms: 100,
    };
    let transaction = store.with_context(&ctx);
    if let Ok(current) = transaction.load() {
        if current.arm == arm && current.mode == Mode::Enabled {
            let mut next = current.clone();
            if let Some(revision) = current.revision.checked_add(1) {
                next.revision = revision;
                next.mode = Mode::Blocked;
                next.phase = Phase::StartInFlight;
                next.code = Code::StartUncertain;
                let _ = transaction.compare_and_set(current.revision, &next);
            }
        }
    }
}

pub(super) fn resume(configuration: &Path, ctx: &Context<'_>) -> PortResult<i32> {
    require_platform()?;
    let store = open(configuration, ctx)?;
    let binding = store.binding_with_context(ctx)?;
    let service = spec(&binding);
    registration_marker::verify(&binding)?;
    artifact::verify(&binding.artifact, &service.render()?)?;
    let process = process();
    service_runtime::check_binding(&binding, &process, ctx)?;
    let owner = store.try_owner_with_context(ctx)?; // No re-arm while another owner/cleanup lives.
    let armed = policy::rearm(&store.with_context(ctx), ctx)?;
    drop(owner); // The service child must be able to acquire its owner lease.
    let platform = Launchd { process: &process };
    if let Err(error) = platform.kickstart(&service, ctx) {
        block_arm(&store, armed.arm);
        return Err(error);
    }
    // A platform acknowledgement is not a running-supervisor proof. No retry.
    // Every failure after dispatch attempts to disarm this arm, including a
    // failed observation (not only a timeout/cancellation).
    let observed = (|| -> PortResult<i32> {
        let until = ctx
            .deadline_monotonic_ms
            .min(ctx.clock.monotonic_ms().saturating_add(10_000));
        loop {
            ctx.check()?;
            let current = store.load_with_context(ctx)?;
            if current.arm != armed.arm || current.mode == Mode::Stopped {
                return Err(Failure::before("supervision_resume_superseded"));
            }
            if current.mode == Mode::Blocked {
                return Err(Failure::before("supervision_start_blocked"));
            }
            if current.phase == Phase::Monitoring {
                match store.try_owner_with_context(ctx) {
                    Err(error) if error.code == "supervision_owner_busy" => {
                        println!(
                            "{}",
                            json!({"status":"supervisor_running","arm":armed.arm,
                            "runtime_ready_verified":false,"state":status_record(&current)})
                        );
                        return Ok(0);
                    }
                    Ok(_owner) => {}
                    Err(error) => return Err(error),
                }
            }
            if ctx.clock.monotonic_ms() >= until {
                return Err(Failure::uncertain("supervision_start_unverified"));
            }
            ctx.wait(20)?;
        }
    })();
    if observed.is_err() {
        block_arm(&store, armed.arm);
    }
    observed.map_err(Failure::after_dispatch)
}

pub(super) fn uninstall(configuration: &Path, ctx: &Context<'_>) -> PortResult<i32> {
    require_platform()?;
    let store = open(configuration, ctx)?;
    let initial = store.load_with_context(ctx)?;
    if initial.registration == Registration::Removed {
        let owner = store.try_owner_with_context(ctx)?;
        let binding = store.binding_with_context(ctx)?;
        // A durable Removed receipt allows only this exact local marker cleanup;
        // it never authorizes another OS unload.
        match registration_marker::path(&binding)?.symlink_metadata() {
            Ok(_) => registration_marker::remove(&binding)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Failure::before("service_marker_unreadable")),
        }
        store.purge_removed(&owner)?;
        println!(
            "{}",
            json!({"status":"uninstalled","identities_preserved":true})
        );
        return Ok(0);
    }
    let fence = stop_store(&store, ctx)?;
    remove_stopped(&store, &fence, &process(), ctx)
}

pub(super) fn remove_stopped(
    store: &SupervisionStore,
    fence: &policy::StopFence,
    process: &dyn tv_adapters::process::Process,
    ctx: &Context<'_>,
) -> PortResult<i32> {
    let _owner = store.try_owner_with_context(ctx)?;
    let binding = store.binding_with_context(ctx)?;
    let current = store.load_with_context(ctx)?;
    if current.arm != fence.arm
        || current.registration != Registration::Installed
        || current.mode != Mode::Stopped
        || current.phase != Phase::Idle
        || current.code != Code::CleanupVerified
    {
        return Err(Failure::uncertain("supervision_cleanup_unverified"));
    }
    let service = spec(&binding);
    let expected = service.render()?;
    registration_marker::verify(&binding)?;
    artifact::verify(&binding.artifact, &expected)?;
    ctx.check()?;
    Launchd { process }.bootout(&service, ctx)?;
    ctx.check().map_err(|error| error.after_dispatch())?;
    if store.load_with_context(ctx)? != current {
        return Err(Failure::uncertain("supervision_uninstall_state_changed"));
    }
    artifact::remove(&binding.artifact, &expected)?;
    publish_registration(
        store,
        &current,
        Registration::Removed,
        Code::CleanupVerified,
        ctx,
    )
    .map_err(|error| error.after_dispatch())?;
    registration_marker::remove(&binding)?;
    store.purge_removed(&_owner)?;
    println!(
        "{}",
        json!({"status":"uninstalled","identities_preserved":true,"journal_preserved":true})
    );
    Ok(0)
}
