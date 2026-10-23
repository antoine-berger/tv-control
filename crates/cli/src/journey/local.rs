//! Local configuration, private ADB access, and MCP readiness ports.
use super::runtime::{
    adb_spec, private_read_limit, resolve_tunnel, verify_adb_dependency, RuntimeIntent,
};
use super::{bounded, gate, Journey};
use tv_adapters::{
    connection_proof::random_token,
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
                    if (OfficialTunnel { process: &process })
                        .status(&spec, &bounded(ctx, 5_000))
                        .is_ok_and(|s| s.locally_ready())
                    {
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
                let mut state = self
                    .runtime
                    .borrow_mut()
                    .as_mut()
                    .ok_or("adb_runtime_missing")?
                    .connect(&bounded(ctx, 10_000))
                    .map_err(|e| e.code)?;
                while state != AdbAuthorization::Authorized {
                    eprintln!("Approve this computer on the TV. Its private controller stays connected while you approve.");
                    self.ask(
                        "Press Enter after TV approval, or q to resume later: ",
                        false,
                        ctx,
                    )?;
                    state = self
                        .runtime
                        .borrow_mut()
                        .as_mut()
                        .ok_or("adb_runtime_missing")?
                        .authorization(&bounded(ctx, 5_000))
                        .map_err(|e| e.code)?;
                }
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
