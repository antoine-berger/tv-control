//! Local configuration, private ADB access, and MCP readiness ports.
use super::runtime::{
    adb_spec, private_read_limit, resolve_tunnel, verify_adb_dependency, RuntimeIntent,
};
use super::{bounded, gate, Journey};
use tv_adapters::{
    connection_proof::{random_token, ProofStore},
    onboarding_runtime::{probe_local_mcp, AdbAuthorization, OwnedAdbRuntime},
    onboarding_tunnel::OfficialTunnel,
    process::BoundedProcess,
};
use tv_application::{
    pipeline::{AdbSetupPort, ConfigurationPort, LocalMcpPort},
    Context,
};
use tv_domain::pipeline::{
    Configuration, Consent, ConsentScope, DeviceBinding, Fresh, Gate, McpReadiness, ReadOnlyAccess,
    Stage,
};

// ADB itself can wait ten seconds for its handshake. Leave bounded room for
// the acknowledgement and subsequent state probe; TV command budgets are unchanged.
const CONNECTION_ATTEMPT_MS: u64 = 12_000;

fn connection_outcome(
    result: tv_domain::PortResult<AdbAuthorization>,
    ctx: &Context<'_>,
) -> Result<AdbAuthorization, &'static str> {
    ctx.check().map_err(|error| error.code)?;
    match result {
        Ok(state) => Ok(state),
        Err(error) if error.code == "expired" => {
            eprintln!("The ADB connection attempt timed out; TV approval was not confirmed.");
            Ok(AdbAuthorization::Offline)
        }
        Err(error) => Err(error.code),
    }
}

impl Journey<'_> {
    fn connection_attempt(
        &self,
        retry: bool,
        ctx: &Context<'_>,
    ) -> Result<AdbAuthorization, &'static str> {
        let attempt = bounded(ctx, CONNECTION_ATTEMPT_MS);
        let mut owned = self.runtime.borrow_mut();
        let runtime = owned.as_mut().ok_or("adb_runtime_missing")?;
        let result = if retry {
            runtime.reconnect(&attempt)
        } else {
            runtime.connect(&attempt)
        };
        // A local attempt timeout is resumable, but never suppress the caller's
        // cancellation or overall deadline to offer another connection attempt.
        ctx.check().map_err(|error| error.code)?;
        if result != Ok(AdbAuthorization::Authorized) {
            if let Some(diagnostic) = runtime.connection_diagnostic() {
                eprintln!("ADB: {diagnostic}");
            }
        }
        connection_outcome(result, ctx)
    }
}

impl ConfigurationPort for Journey<'_> {
    fn configuration(&self, _: &Context<'_>) -> Gate<Configuration> {
        Gate::Observed(self.configuration.clone())
    }
}
impl AdbSetupPort for Journey<'_> {
    fn consent(&self, config: &Configuration, ctx: &Context<'_>) -> Gate<Fresh<Consent>> {
        gate((|| {
            self.progress(Stage::AdbConsent, "[1/4] TV access", ctx)?;
            if !self.adb_approved.get() {
                if self.ask(
                    "Allow a private ADB identity and connection to this TV? [y/N] ",
                    false,
                    ctx,
                )? != "y"
                {
                    return Err("approval_rejected");
                }
                self.adb_approved.set(true);
            }
            Ok(self.fresh(self.consent(config, ConsentScope::DedicatedAdb, ctx), ctx))
        })())
    }
    fn owned_runtime(&self, _: &Consent, ctx: &Context<'_>) -> Gate<Fresh<DeviceBinding>> {
        gate((|| {
            verify_adb_dependency(&self.config, &self.state, ctx)?;
            let id = self.checkpoint.borrow().tunnel_id.clone();
            if let (Some(id), Some(binary)) = (id, resolve_tunnel(&self.state, ctx)?) {
                if let Ok(bytes) =
                    private_read_limit(&self.state.join("runtime-intent.json"), 6_400_000)
                {
                    let intent: RuntimeIntent =
                        serde_json::from_slice(&bytes).map_err(|_| "runtime_intent_invalid")?;
                    if intent.configuration.as_bytes() != self.snapshot
                        || intent.installation_id != self.installation
                        || intent.device != self.configuration.device
                    {
                        return Err("runtime_intent_mismatch");
                    }
                    if intent.controller_binary != self.controller_binary {
                        return Err("runtime_launcher_changed");
                    }
                    let spec = self.tunnel_spec(binary, id)?;
                    let process = BoundedProcess {
                        max_output: 65_536,
                        poll_ms: 10,
                    };
                    let adapter = OfficialTunnel { process: &process };
                    match adapter.status(&spec, &bounded(ctx, 5_000)) {
                        Ok(health) if health.locally_ready() => {
                            self.existing_runtime.set(true);
                            *self.tunnel.borrow_mut() = Some(spec);
                            return Ok(self.fresh(
                                DeviceBinding {
                                    configuration: self.configuration.clone(),
                                    runtime_generation: self.bridge_generation()?,
                                },
                                ctx,
                            ));
                        }
                        Ok(health) if health.process_running => {
                            eprintln!(
                                "The existing controller is running but unavailable ({}). Restarting it preserves the TV identity and configuration; tunnel access will be requested again.",
                                health.mcp_failure_code.unwrap_or("tunnel_runtime_not_ready")
                            );
                            if self.ask(
                                "Stop this owned controller and retry with the installed version? [y/N] ",
                                false,
                                ctx,
                            )? != "y"
                            {
                                return Err("approval_rejected");
                            }
                            let stopped = adapter
                                .stop(&spec, true, &bounded(ctx, 10_000))
                                .map_err(|error| error.code)?;
                            let checked = adapter
                                .status(&spec, &bounded(ctx, 5_000))
                                .map_err(|error| error.code)?;
                            if stopped.process_running || checked.process_running {
                                return Err("controller_stop_unverified");
                            }
                            ProofStore::open(&self.state.join("verification"))?.invalidate()?;
                        }
                        Ok(_) => {}
                        Err(error) if error.code == "tunnel_state_missing" => {}
                        Err(error) => return Err(error.code),
                    }
                }
            }
            let runtime = OwnedAdbRuntime::start(
                adb_spec(&self.config, &self.configuration.device, &self.state)?,
                self.adb_approved.get(),
                &bounded(ctx, 10_000),
            )
            .map_err(|e| e.code)?;
            *self.runtime.borrow_mut() = Some(runtime);
            Ok(self.fresh(
                DeviceBinding {
                    configuration: self.configuration.clone(),
                    runtime_generation: random_token()?,
                },
                ctx,
            ))
        })())
    }
    fn observe(&self, binding: &DeviceBinding, ctx: &Context<'_>) -> Gate<Fresh<ReadOnlyAccess>> {
        gate((|| {
            if !self.existing_runtime.get() {
                let mut state = self.connection_attempt(false, ctx)?;
                while state != AdbAuthorization::Authorized {
                    let prompt = match state {
                        AdbAuthorization::AwaitingAuthorization => {
                            eprintln!("The TV has not authorized this private controller. Check for a debugging authorization dialog.");
                            "Press Enter after TV approval, type retry to reconnect this TV, or q to resume later: "
                        }
                        AdbAuthorization::Connecting => {
                            eprintln!("ADB is still establishing the connection; TV approval is not yet confirmed.");
                            "Press Enter to check connection, type retry to reconnect this TV, or q to resume later: "
                        }
                        AdbAuthorization::Offline | AdbAuthorization::Absent => {
                            eprintln!("ADB has not confirmed a usable connection to the configured TV. Check its address, network debugging and TV settings.");
                            "Press Enter to retry connection, or q to resume later: "
                        }
                        AdbAuthorization::Authorized => unreachable!(),
                    };
                    let response = self.ask(prompt, false, ctx)?;
                    if response != "retry" && !response.is_empty() {
                        continue;
                    }
                    state = if response == "retry"
                        || matches!(state, AdbAuthorization::Offline | AdbAuthorization::Absent)
                    {
                        self.connection_attempt(true, ctx)?
                    } else {
                        self.runtime
                            .borrow_mut()
                            .as_mut()
                            .ok_or("adb_runtime_missing")?
                            .authorization(&bounded(ctx, 5_000))
                            .map_err(|e| e.code)?
                    };
                }
            }
            if self.config.generic_adb_control {
                use tv_application::generic::Device;
                let parent = self.path.parent().ok_or("configuration_path_invalid")?;
                let device = tv_adapters::generic_adb::GenericAdb::new(
                    self.config.clone(),
                    std::sync::Arc::new(BoundedProcess {
                        max_output: 8 * 1024 * 1024,
                        poll_ms: self.config.preferences.process_poll_ms,
                    }),
                    std::sync::Arc::new(tv_adapters::adb_server::SmartSocketProbe {
                        poll_ms: self.config.preferences.process_poll_ms,
                    }),
                )
                .with_cache_dir(parent.join(".tv-control-catalogue"));
                eprintln!("Discovering installed TV apps (read-only)...");
                match device.apps(&self.configuration.device, &bounded(ctx, 30_000)) {
                    Ok(apps) => {
                        let labels = apps.iter().filter(|app|app.label.is_some()).count();
                        eprintln!("Discovered {} apps; {} display names available. Missing names remain available by app ID.",apps.len(),labels);
                    }
                    Err(error) => eprintln!("App catalogue is not ready ({}). Discovery can resume from the conversation; connection checks continue.",error.code),
                }
                ctx.check().map_err(|error| error.code)?;
            }
            let receipt = self.observed(random_token()?, &bounded(ctx, 10_000))?;
            Ok(self.fresh(
                ReadOnlyAccess {
                    binding: binding.clone(),
                    receipt,
                },
                ctx,
            ))
        })())
    }
}
impl LocalMcpPort for Journey<'_> {
    fn initialize_and_list(
        &self,
        binding: &DeviceBinding,
        ctx: &Context<'_>,
    ) -> Gate<Fresh<McpReadiness>> {
        gate((|| {
            self.progress(Stage::LocalMcp, "[2/4] Local controller", ctx)?;
            probe_local_mcp(
                &std::env::current_exe().map_err(|_| "executable_unavailable")?,
                &self.path,
                &self.snapshot,
                &self.configuration.device,
                &bounded(ctx, 5_000),
            )
            .map_err(|e| e.code)?;
            Ok(self.fresh(
                McpReadiness {
                    binding: binding.clone(),
                    initialized: true,
                    observe_tool_available: true,
                },
                ctx,
            ))
        })())
    }
}

#[cfg(test)]
mod tests {
    use super::connection_outcome;
    use tv_adapters::{
        onboarding_runtime::AdbAuthorization,
        runtime::{CancelFlag, SystemClock},
    };
    use tv_application::Context;
    use tv_domain::Failure;

    #[test]
    fn only_local_attempt_expiry_is_retryable() {
        let clock = SystemClock::default();
        let cancel = CancelFlag::default();
        let ctx = Context {
            clock: &clock,
            cancellation: &cancel,
            deadline_monotonic_ms: 10_000,
        };
        assert_eq!(
            connection_outcome(Err(Failure::before("expired")), &ctx),
            Ok(AdbAuthorization::Offline)
        );
        assert_eq!(
            connection_outcome(Err(Failure::before("adb_server_protocol_mismatch")), &ctx),
            Err("adb_server_protocol_mismatch")
        );
        let expired = Context {
            deadline_monotonic_ms: 0,
            ..ctx
        };
        assert_eq!(
            connection_outcome(Ok(AdbAuthorization::Authorized), &expired),
            Err("expired")
        );
        assert_eq!(
            connection_outcome(Err(Failure::before("expired")), &expired),
            Err("expired")
        );
        cancel.cancel();
        assert_eq!(
            connection_outcome(Err(Failure::before("expired")), &ctx),
            Err("cancelled")
        );
        assert_eq!(
            connection_outcome(Ok(AdbAuthorization::Authorized), &ctx),
            Err("cancelled")
        );
    }
}
