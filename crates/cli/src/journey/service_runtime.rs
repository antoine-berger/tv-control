//! Existing-access backend for the opt-in host supervisor. Never accepts a TV request.
mod binding;
#[cfg(test)]
mod tests;
use binding::Existing;
use std::{
    net::{SocketAddr, TcpStream},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};
use tv_adapters::{
    onboarding_tunnel::{OfficialTunnel, RuntimeHealth},
    pipeline_store::Store as PipelineStore,
    process::Process,
    runtime_credential, runtime_intent,
    supervision_store::{Binding, SupervisionStore},
};
use tv_application::{
    supervision::{Backend, Code, Mode, Observation, Phase, Registration},
    Cancellation, Context,
};
use tv_domain::{Effect, Failure, PortResult};

fn fail(code: &'static str) -> Failure {
    Failure::before(code)
}

pub(crate) fn capture_existing(
    configuration: &Path,
    artifact: &Path,
    helper: &Path,
    uid: u32,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> PortResult<Binding> {
    binding::capture(configuration, artifact, helper, uid, process, ctx)
}
pub(crate) fn check_binding(
    value: &Binding,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> PortResult<()> {
    binding::existing(value, process, ctx).map(|_| ())
}
/// Read-only planning. No directory, credential, runtime, registration or artifact is created.
pub(crate) fn plan_binding(configuration: &Path, ctx: &Context<'_>) -> PortResult<Binding> {
    let (installation, helper) = binding::coordinates(configuration, ctx)?;
    let home = std::env::var_os("HOME").ok_or(fail("supervision_home_unavailable"))?;
    let artifact = Path::new(&home)
        .join("Library/LaunchAgents")
        .join(format!("com.tv-control.{installation}.plist"));
    let process = tv_adapters::process::BoundedProcess {
        max_output: 65_536,
        poll_ms: 10,
    };
    capture_existing(
        configuration,
        &artifact,
        &helper,
        unsafe { libc::geteuid() },
        &process,
        ctx,
    )
}

struct Fence<'a> {
    store: &'a SupervisionStore,
    arm: u64,
    outer: Context<'a>,
    start: bool,
    error: Mutex<Option<Failure>>,
}
impl Fence<'_> {
    fn check_record(&self) -> PortResult<()> {
        // This context retains the caller's original cancellation, never this
        // Fence itself: retrying a short store lock cannot recurse into the fence.
        let record = self.store.load_with_context(&self.outer)?;
        if record.arm != self.arm || record.registration != Registration::Installed {
            return Err(fail("supervision_arm_changed"));
        }
        if self.start && record.mode != Mode::Enabled {
            return Err(if record.mode == Mode::Stopped {
                Failure::cancelled("supervision_stopped")
            } else {
                fail("supervision_not_enabled")
            });
        }
        Ok(())
    }
    fn result<T>(&self, result: PortResult<T>) -> PortResult<T> {
        let stored = self
            .error
            .lock()
            .map_err(|_| fail("supervision_fence_unavailable"))?;
        if let Some(error) = *stored {
            return Err(
                if result
                    .as_ref()
                    .is_err_and(|e| e.effect == Effect::MayHaveChanged)
                {
                    error.after_dispatch()
                } else {
                    error
                },
            );
        }
        result
    }
}
impl Cancellation for Fence<'_> {
    fn is_cancelled(&self) -> bool {
        if self.outer.cancellation.is_cancelled() {
            return true;
        }
        if self.error.lock().map_or(true, |error| error.is_some()) {
            return true;
        }
        if let Err(error) = self.check_record() {
            if let Ok(mut stored) = self.error.lock() {
                stored.get_or_insert(error);
            }
            return true;
        }
        false
    }
}

pub(crate) struct RuntimeBackend<'a> {
    store: &'a SupervisionStore,
    arm: u64,
    process: &'a dyn Process,
    unresolved: AtomicBool,
}
impl<'a> RuntimeBackend<'a> {
    pub(crate) fn new_with_context(
        store: &'a SupervisionStore,
        arm: u64,
        process: &'a dyn Process,
        ctx: &Context<'_>,
    ) -> PortResult<Self> {
        ctx.check()?;
        let record = store.load_with_context(ctx)?;
        if record.arm != arm {
            return Err(fail("supervision_arm_changed"));
        }
        let unresolved = matches!(record.phase, Phase::StartInFlight | Phase::CleanupInFlight)
            || record.code == Code::StartUncertain;
        ctx.check()?;
        Ok(Self {
            store,
            arm,
            process,
            unresolved: AtomicBool::new(unresolved),
        })
    }
    fn existing(&self, ctx: &Context<'_>) -> PortResult<Existing> {
        ctx.check()?;
        let record = self.store.load_with_context(ctx)?;
        if record.arm != self.arm {
            return Err(fail("supervision_arm_changed"));
        }
        binding::existing(&self.store.binding_with_context(ctx)?, self.process, ctx)
    }
    fn generation(existing: &Existing) -> PortResult<String> {
        runtime_intent::read_generation(
            &existing.state,
            &existing.intent.installation_id,
            &existing.intent.revision,
            &existing.intent.device,
        )
        .map_err(fail)
    }
    fn port_closed(existing: &Existing, ctx: &Context<'_>) -> PortResult<()> {
        ctx.check()?;
        let spec =
            super::runtime::adb_spec(&existing.config, &existing.intent.device, &existing.state)
                .map_err(fail)?;
        let address = SocketAddr::from(([127, 0, 0, 1], spec.port));
        let result = TcpStream::connect_timeout(
            &address,
            Duration::from_millis(ctx.remaining_ms().clamp(1, 50)),
        );
        ctx.check()?;
        match result {
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => Ok(()),
            Err(error) => {
                Err(fail("supervision_adb_absence_unverified").with_os_error(error.raw_os_error()))
            }
            Ok(_) => Err(fail("supervision_unverified_adb_orphan")),
        }
    }
    fn observation(
        existing: &Existing,
        health: RuntimeHealth,
        ctx: &Context<'_>,
    ) -> PortResult<Observation> {
        ctx.check()?;
        if !health.process_running {
            Self::port_closed(existing, ctx)?;
            return Ok(Observation::Absent);
        }
        if !health.locally_ready() {
            return Ok(Observation::Degraded);
        }
        Self::generation(existing)?;
        ctx.check()?;
        // Fresh official host liveness and a compatible bridge generation, not
        // device connectivity, physical playback or external tunnel availability.
        Ok(Observation::Ready)
    }
}
impl Backend for RuntimeBackend<'_> {
    fn observe_owned(&self, ctx: &Context<'_>) -> PortResult<Observation> {
        let existing = self.existing(ctx)?;
        let health = OfficialTunnel {
            process: self.process,
        }
        .status(&existing.spec, ctx)?;
        Self::observation(&existing, health, ctx)
    }
    fn start_owned(&self, ctx: &Context<'_>) -> PortResult<Observation> {
        if self.unresolved.load(Ordering::Acquire) {
            return Err(fail("supervision_start_unresolved"));
        }
        let existing = self.existing(ctx)?;
        // The pipeline lease serializes official lifecycle calls. The short
        // supervision transaction lock is never retained while this is acquired.
        let _pipeline = PipelineStore::open(&existing.state).map_err(fail)?;
        let fence = Fence {
            store: self.store,
            arm: self.arm,
            outer: Context {
                clock: ctx.clock,
                cancellation: ctx.cancellation,
                deadline_monotonic_ms: ctx.deadline_monotonic_ms,
            },
            start: true,
            error: Mutex::new(None),
        };
        fence.check_record()?;
        let record = self.store.load_with_context(ctx)?;
        if record.phase != Phase::StartInFlight {
            return Err(fail("supervision_start_not_admitted"));
        }
        let fenced = Context {
            clock: ctx.clock,
            cancellation: &fence,
            deadline_monotonic_ms: ctx.deadline_monotonic_ms,
        };
        let result = (|| {
            let existing = self.existing(&fenced)?;
            let tunnel = OfficialTunnel {
                process: self.process,
            };
            let before = tunnel.status(&existing.spec, &fenced)?;
            if before.process_running {
                return Err(fail("supervision_runtime_not_absent"));
            }
            Self::port_closed(&existing, &fenced)?;
            let previous = match Self::generation(&existing) {
                Ok(value) => Some(value),
                Err(error) if error.code == "bridge_generation_unavailable" => None,
                Err(error) => return Err(error),
            };
            let key = runtime_credential::load(
                &existing.state,
                &existing.intent.installation_id,
                &existing.spec.tunnel_id,
            )
            .map_err(fail)?
            .ok_or(fail("supervision_saved_credential_required"))?;
            // Recheck the binding and durable arm after preparation, immediately
            // before the existing ensure path may dispatch a single start.
            self.existing(&fenced)?;
            fenced.check()?;
            self.unresolved.store(true, Ordering::Release);
            let health = match tunnel.ensure(&existing.spec, true, Some(key.as_ref()), &fenced) {
                Ok(value) => value,
                Err(error) => {
                    if error.effect == Effect::None {
                        self.unresolved.store(false, Ordering::Release);
                    }
                    return Err(error);
                }
            };
            if !health.locally_ready() {
                return Err(Failure::uncertain("supervision_start_not_ready"));
            }
            loop {
                fenced.check().map_err(Failure::after_dispatch)?;
                match Self::generation(&existing) {
                    Ok(value) if Some(&value) != previous.as_ref() => break,
                    Ok(_) => {}
                    Err(error) if error.code == "bridge_generation_unavailable" => {}
                    Err(error) => return Err(error.after_dispatch()),
                }
                fenced.wait(20).map_err(Failure::after_dispatch)?;
            }
            self.existing(&fenced).map_err(Failure::after_dispatch)?;
            fenced.check().map_err(Failure::after_dispatch)?;
            self.unresolved.store(false, Ordering::Release);
            Ok(Observation::Ready)
        })();
        fence.result(result)
    }
    fn stop_owned(&self, ctx: &Context<'_>) -> PortResult<Observation> {
        // A snapshot saying absent cannot prove that an old detached start has
        // finished. Version one deliberately blocks this unresolved cleanup.
        if self.unresolved.load(Ordering::Acquire) {
            return Err(fail("supervision_cleanup_unverified"));
        }
        let existing = self.existing(ctx)?;
        let _pipeline = PipelineStore::open(&existing.state).map_err(fail)?;
        let fence = Fence {
            store: self.store,
            arm: self.arm,
            outer: Context {
                clock: ctx.clock,
                cancellation: ctx.cancellation,
                deadline_monotonic_ms: ctx.deadline_monotonic_ms,
            },
            start: false,
            error: Mutex::new(None),
        };
        fence.check_record()?;
        let fenced = Context {
            clock: ctx.clock,
            cancellation: &fence,
            deadline_monotonic_ms: ctx.deadline_monotonic_ms,
        };
        let result = (|| {
            let existing = self.existing(&fenced)?;
            let tunnel = OfficialTunnel {
                process: self.process,
            };
            let before = tunnel.status(&existing.spec, &fenced)?;
            if before.process_running {
                tunnel.stop(&existing.spec, true, &fenced)?;
            }
            let after = tunnel.status(&existing.spec, &fenced)?;
            if after.process_running {
                return Err(Failure::uncertain("supervision_cleanup_unverified"));
            }
            Self::port_closed(&existing, &fenced).map_err(Failure::after_dispatch)?;
            self.existing(&fenced).map_err(Failure::after_dispatch)?;
            fenced.check().map_err(Failure::after_dispatch)?;
            Ok(Observation::Absent)
        })();
        fence.result(result)
    }
}
