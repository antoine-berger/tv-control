//! Composition of the guided journey. Account approvals remain explicit and resumable.
use crate::terminal::{Interrupts, Terminal};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use tv_adapters::{
    config::{Config, Device},
    connection_proof::{random_token, Challenge, ProofStore},
    onboarding_runtime::{probe_local_mcp, AdbAuthorization, AdbRuntimeSpec, OwnedAdbRuntime},
    onboarding_tunnel::{OfficialTunnel, TunnelSpec},
    pipeline_store::{Checkpoint, EventCode, LogEvent, ProgressOutcome, ProgressStage, Store},
    process::BoundedProcess,
    runtime::SystemClock,
};
use tv_application::{pipeline::*, Clock, Context};
use tv_domain::{pipeline::*, Action, Receipt, Request, Status};

const EVIDENCE_MS: u64 = 900_000;
const JOURNEY_MS: u64 = 1_800_000;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeIntent {
    schema: u32,
    installation_id: String,
    revision: String,
    device: String,
    configuration: String,
    controller_binary: PathBuf,
}
fn private_read(path: &Path) -> Result<Vec<u8>, &'static str> {
    private_read_limit(path, 1_048_576)
}
fn private_read_limit(path: &Path, limit: usize) -> Result<Vec<u8>, &'static str> {
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "runtime_intent_unreadable")?;
    let m = f.metadata().map_err(|_| "runtime_intent_unreadable")?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.nlink() != 1
        || m.mode() & 0o077 != 0
    {
        return Err("runtime_intent_not_private");
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut f)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "runtime_intent_unreadable")?;
    if bytes.len() > limit {
        return Err("runtime_intent_too_large");
    }
    Ok(bytes)
}
fn adb_spec(cfg: &Config, alias: &str, state: &Path) -> Result<AdbRuntimeSpec, &'static str> {
    match cfg.devices.get(alias) {
        Some(Device::Adb {
            endpoint,
            server_host,
            server_port,
            ..
        }) if server_host == "127.0.0.1" => Ok(AdbRuntimeSpec {
            executable: cfg.executables.adb.clone(),
            state_dir: state.join("adb"),
            port: *server_port,
            endpoint: endpoint.clone(),
            poll_ms: cfg.preferences.process_poll_ms,
        }),
        _ => Err("onboarding_requires_dedicated_adb_device"),
    }
}
fn bounded<'a>(ctx: &'a Context<'a>, ms: u64) -> Context<'a> {
    Context {
        clock: ctx.clock,
        cancellation: ctx.cancellation,
        deadline_monotonic_ms: ctx
            .deadline_monotonic_ms
            .min(ctx.clock.monotonic_ms().saturating_add(ms)),
    }
}
fn stopped(code: &'static str) -> StopReason {
    match code {
        "cancelled" => StopReason::Cancelled,
        "expired" => StopReason::Expired,
        "approval_rejected" => StopReason::Rejected,
        "tv_authorization_pending" => StopReason::Pending,
        _ => StopReason::Blocked(code),
    }
}
fn gate<T>(result: Result<T, &'static str>) -> Gate<T> {
    match result {
        Ok(v) => Gate::Observed(v),
        Err(e) => Gate::Stopped(stopped(e)),
    }
}
fn find_program(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|p| p.is_absolute())
        .map(|p| p.join(name))
        .find_map(|p| {
            let m = fs::metadata(&p).ok()?;
            (m.is_file() && m.permissions().mode() & 0o111 != 0)
                .then(|| p.canonicalize().ok())
                .flatten()
        })
}
struct Journey<'a> {
    path: PathBuf,
    state: PathBuf,
    snapshot: Vec<u8>,
    config: Arc<Config>,
    configuration: Configuration,
    controller_binary: PathBuf,
    installation: String,
    terminal: &'a Terminal,
    store: RefCell<Store>,
    checkpoint: RefCell<Checkpoint>,
    adb_approved: Cell<bool>,
    tunnel_approved: Cell<bool>,
    runtime: RefCell<Option<OwnedAdbRuntime>>,
    existing_runtime: Cell<bool>,
    tunnel: RefCell<Option<TunnelSpec>>,
    key: RefCell<Option<String>>,
    challenge: RefCell<Option<Challenge>>,
}
impl Journey<'_> {
    fn fresh<T>(&self, value: T, ctx: &Context<'_>) -> Fresh<T> {
        let now = ctx.clock.unix_ms();
        Fresh {
            value,
            observed_at_unix_ms: now,
            expires_at_unix_ms: now.saturating_add(EVIDENCE_MS),
            mode: EvidenceMode::Live,
        }
    }
    fn progress(&self, stage: Stage, title: &str, ctx: &Context<'_>) -> Result<(), &'static str> {
        eprintln!("{title}");
        let mut state = self.checkpoint.borrow_mut();
        state.stage = stage.into();
        state.outcome = ProgressOutcome::Pending;
        let mut store = self.store.borrow_mut();
        store.save(&state)?;
        store.event(LogEvent {
            stage: stage.into(),
            code: EventCode::Checking,
            elapsed_ms: ctx.clock.monotonic_ms(),
        })
    }
    fn ask(&self, prompt: &str, secret: bool, ctx: &Context<'_>) -> Result<String, &'static str> {
        self.terminal.line(prompt, secret, ctx)?.ok_or("cancelled")
    }
    fn consent(&self, config: &Configuration, scope: ConsentScope, ctx: &Context<'_>) -> Consent {
        Consent {
            configuration: config.clone(),
            scope,
            approval_reference: format!(
                "interactive-{}-{}",
                self.installation,
                ctx.clock.unix_ms()
            ),
        }
    }
    fn observed(&self, id: String, ctx: &Context<'_>) -> Result<Receipt, &'static str> {
        if private_read(&self.path)? != self.snapshot {
            return Err("configuration_changed_restart_setup");
        }
        let receipt = super::service(self.config.clone()).execute(
            Request {
                id,
                device: self.configuration.device.clone(),
                action: Action::Observe,
                expires_at_unix_ms: ctx
                    .clock
                    .unix_ms()
                    .saturating_add(ctx.remaining_ms().min(10_000)),
            },
            ctx.cancellation,
        );
        if receipt.status != Status::Observed
            || receipt.cached
            || !receipt.observed.as_ref().is_some_and(|s| s.connected)
        {
            return Err("tv_observation_unverified");
        }
        if private_read(&self.path)? != self.snapshot {
            return Err("configuration_changed_restart_setup");
        }
        Ok(receipt)
    }
    fn tunnel_spec(&self, binary: PathBuf, id: String) -> Result<TunnelSpec, &'static str> {
        Ok(TunnelSpec {
            binary,
            alias: format!("tv-{}", self.installation),
            tunnel_id: id,
            runtime_key_env: "TV_CONTROL_TUNNEL_KEY".into(),
            state_dir: self.state.join("tunnel"),
            mcp_binary: self.controller_binary.clone(),
            configuration: self.path.clone(),
            bridge_state_dir: Some(self.state.clone()),
            executable_path: std::env::var_os("PATH").ok_or("executable_path_missing")?,
        })
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
            let id = self.checkpoint.borrow().tunnel_id.clone();
            if let (Some(id), Some(binary)) = (id, find_program("tunnel-client")) {
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
                                runtime_generation: self.installation.clone(),
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
                    runtime_generation: self.installation.clone(),
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
impl TunnelSetupPort for Journey<'_> {
    fn consent(&self, config: &Configuration, ctx: &Context<'_>) -> Gate<Fresh<Consent>> {
        gate((|| {
            self.progress(Stage::TunnelConsent, "[3/4] ChatGPT connection", ctx)?;
            let binary = loop {
                if let Some(binary) = find_program("tunnel-client") {
                    break binary;
                }
                eprintln!("Download the official tunnel-client from https://platform.openai.com/settings/organization/tunnels");
                self.ask(
                    "Press Enter when installed, or q to resume later: ",
                    false,
                    ctx,
                )?;
            };
            let saved = self.checkpoint.borrow().tunnel_id.clone();
            let id = match saved {
                Some(id) => id,
                None => {
                    eprintln!("In Platform, select your organization/workspace and reuse or explicitly create a tunnel. Runtime access needs Tunnels Read + Use.");
                    self.ask("Tunnel ID: ", false, ctx)?
                }
            };
            if !self.tunnel_approved.get() {
                if self.ask("Allow a background TV controller through this tunnel until you stop it? [y/N] ",false,ctx)? != "y" {return Err("approval_rejected");}
                self.tunnel_approved.set(true);
            }
            if !self.existing_runtime.get() {
                let key = match std::env::var("TV_CONTROL_TUNNEL_KEY") {
                    Ok(v) if !v.is_empty() => v,
                    _ => self.ask("Runtime key (hidden; not saved): ", true, ctx)?,
                };
                if key.is_empty() || key.chars().any(char::is_control) {
                    return Err("runtime_key_invalid");
                }
                *self.key.borrow_mut() = Some(key);
            }
            let alias = format!("tv-{}", self.installation);
            let spec = self.tunnel_spec(binary, id.clone())?;
            spec.validate().map_err(|e| e.code)?;
            let mut checkpoint = self.checkpoint.borrow_mut();
            checkpoint.tunnel_id = Some(id);
            checkpoint.tunnel_alias = Some(alias);
            self.store.borrow_mut().save(&checkpoint)?;
            *self.tunnel.borrow_mut() = Some(spec);
            Ok(self.fresh(self.consent(config, ConsentScope::TunnelAccess, ctx), ctx))
        })())
    }
    fn readiness(
        &self,
        binding: &DeviceBinding,
        _: &Consent,
        ctx: &Context<'_>,
    ) -> Gate<Fresh<ConnectionBinding>> {
        gate((|| {
            ctx.check().map_err(|e| e.code)?;
            let intent = RuntimeIntent {
                schema: 1,
                installation_id: self.installation.clone(),
                revision: self.configuration.revision.clone(),
                device: self.configuration.device.clone(),
                configuration: String::from_utf8(self.snapshot.clone())
                    .map_err(|_| "configuration_invalid_utf8")?,
                controller_binary: self.controller_binary.clone(),
            };
            let bytes = serde_json::to_vec(&intent).map_err(|_| "runtime_intent_encode_failed")?;
            tv_adapters::runtime_intent::publish(&self.state, &bytes)?;
            // The official manager supervises bridge, which then owns ADB for its lifetime.
            self.runtime.borrow_mut().take();
            let process = BoundedProcess {
                max_output: 65_536,
                poll_ms: 10,
            };
            let spec = self.tunnel.borrow();
            let spec = spec.as_ref().ok_or("tunnel_configuration_missing")?;
            let key = self.key.borrow_mut().take();
            let health = OfficialTunnel { process: &process }
                .ensure(
                    spec,
                    self.tunnel_approved.get(),
                    key.as_deref().map(OsStr::new),
                    &bounded(ctx, 60_000),
                )
                .map_err(|e| e.code)?;
            if !health.locally_ready() {
                return Err("tunnel_not_ready");
            }
            Ok(self.fresh(
                ConnectionBinding {
                    device: binding.clone(),
                    tunnel_generation: spec.tunnel_id.clone(),
                },
                ctx,
            ))
        })())
    }
}
impl ExternalConversationPort for Journey<'_> {
    fn challenge(
        &self,
        binding: &ConnectionBinding,
        ctx: &Context<'_>,
    ) -> Gate<Fresh<ProbeChallenge>> {
        gate((|| {
            self.progress(
                Stage::ExternalConversation,
                "[4/4] Verify your conversation",
                ctx,
            )?;
            let id = self
                .checkpoint
                .borrow()
                .tunnel_id
                .clone()
                .ok_or("tunnel_configuration_missing")?;
            eprintln!("In ChatGPT: Plugins → Add custom MCP server → Tunnel ({id}). Approve, install and select it in your intended conversation.");
            self.ask(
                "Press Enter when the plugin is selected, or q to resume later: ",
                false,
                ctx,
            )?;
            let challenge = ProofStore::open(&self.state.join("verification"))?.issue(
                &self.installation,
                &self.configuration.revision,
                &self.snapshot,
                &self.configuration.device,
                ctx.clock.unix_ms(),
            )?;
            let nonce = challenge.nonce.clone();
            *self.challenge.borrow_mut() = Some(challenge);
            Ok(self.fresh(
                ProbeChallenge {
                    nonce,
                    binding: binding.clone(),
                },
                ctx,
            ))
        })())
    }
    fn verify(
        &self,
        challenge: &ProbeChallenge,
        ctx: &Context<'_>,
    ) -> Gate<Fresh<ExternalConversationVerification>> {
        gate((|| {
            eprintln!("Ask that conversation: Call tv_verify_connection with challenge {} and return its verification_code.",challenge.nonce);
            eprintln!("This verifies your chosen conversation cooperatively; the tunnel does not attest a signed conversation identity.");
            let answer = self.ask(
                "Verification code returned by that conversation (q resumes later): ",
                false,
                ctx,
            )?;
            let saved = self.challenge.borrow();
            let saved = saved.as_ref().ok_or("verification_challenge_missing")?;
            if private_read(&self.path)? != self.snapshot {
                return Err("configuration_changed_restart_setup");
            }
            let proof = ProofStore::open(&self.state.join("verification"))?.confirm(
                saved,
                &answer,
                &self.snapshot,
                ctx.clock.unix_ms(),
            )?;
            let process = BoundedProcess {
                max_output: 65_536,
                poll_ms: 10,
            };
            let tunnel = self.tunnel.borrow();
            let tunnel = tunnel.as_ref().ok_or("tunnel_configuration_missing")?;
            if !(OfficialTunnel { process: &process })
                .status(tunnel, &bounded(ctx, 5_000))
                .map_err(|e| e.code)?
                .locally_ready()
            {
                return Err("controller_stopped_before_confirmation");
            }
            self.observed(random_token()?, &bounded(ctx, 10_000))?;
            let receipt = proof.receipt;
            let mut fresh = self.fresh(
                ExternalConversationVerification {
                    challenge: challenge.clone(),
                    confirmation_reference: format!("cooperative-{}", self.installation),
                    receipt,
                },
                ctx,
            );
            fresh.observed_at_unix_ms = proof.observed_at_unix_ms;
            Ok(fresh)
        })())
    }
}
impl Drop for Journey<'_> {
    fn drop(&mut self) {
        if let Some(challenge) = self.challenge.get_mut().take() {
            if let Ok(store) = ProofStore::open(&self.state.join("verification")) {
                let _ = store.cancel(&challenge);
            }
        }
    }
}

pub fn run(path: &Path, as_json: bool, launcher: Option<&Path>) -> Result<i32, &'static str> {
    if as_json {
        println!(
            "{}",
            json!({"status":"pending","stage":"approval","code":"interactive_approval_required","ready":false})
        );
        return Ok(2);
    }
    let terminal = Terminal::open()?;
    let interrupts = Interrupts::install()?;
    let clock = SystemClock::default();
    let ctx = Context {
        clock: &clock,
        cancellation: &interrupts,
        deadline_monotonic_ms: JOURNEY_MS,
    };
    let path = path
        .canonicalize()
        .map_err(|_| "configuration_unreadable")?;
    let snapshot = private_read(&path)?;
    let config = Arc::new(Config::from_json(&snapshot)?);
    let controller_binary = match launcher {
        Some(path) => {
            let m = path
                .symlink_metadata()
                .map_err(|_| "controller_launcher_unavailable")?;
            if !path.is_absolute()
                || !m.is_file()
                || m.uid() != unsafe { libc::geteuid() }
                || m.mode() & 0o022 != 0
                || m.mode() & 0o111 == 0
            {
                return Err("controller_launcher_not_private");
            }
            path.to_owned()
        }
        None => std::env::current_exe().map_err(|_| "executable_unavailable")?,
    };
    let parent = path.parent().ok_or("configuration_path_invalid")?;
    let state = parent.join("connection");
    let mut store = Store::open(&state)?;
    let previous = match store.load() {
        Ok(previous) => previous,
        Err("pipeline_checkpoint_corrupt") => {
            if terminal.line("Saved connection progress is damaged. Preserve it and restart these checks? [y/N] ",false,&ctx)?.as_deref()!=Some("y") {return Err("cancelled");}
            store.quarantine_checkpoint()?;
            None
        }
        Err(code) => return Err(code),
    };
    let mut devices = config
        .devices
        .iter()
        .filter(|(_, d)| matches!(d, Device::Adb { .. }));
    let device = devices
        .next()
        .map(|(alias, _)| alias.clone())
        .ok_or("onboarding_requires_adb")?;
    if devices.next().is_some() {
        return Err("onboarding_requires_single_adb_device");
    }
    let installation = previous
        .as_ref()
        .map(|p| p.installation_id.clone())
        .unwrap_or(random_token()?);
    let revision = previous
        .as_ref()
        .map(|p| p.configuration_revision.clone())
        .unwrap_or(random_token()?);
    let checkpoint = Checkpoint {
        schema: 1,
        installation_id: installation.clone(),
        configuration_path: path.clone(),
        configuration_revision: revision.clone(),
        device: device.clone(),
        stage: ProgressStage::Configuration,
        outcome: ProgressOutcome::Pending,
        tunnel_id: previous.as_ref().and_then(|p| p.tunnel_id.clone()),
        tunnel_alias: previous.and_then(|p| p.tunnel_alias),
    };
    store.save(&checkpoint)?;
    let journey = Journey {
        path,
        state,
        snapshot,
        config,
        configuration: Configuration {
            revision,
            device,
            target_conversation: "user-selected-conversation".into(),
        },
        controller_binary,
        installation,
        terminal: &terminal,
        store: RefCell::new(store),
        checkpoint: RefCell::new(checkpoint),
        adb_approved: Cell::new(false),
        tunnel_approved: Cell::new(false),
        existing_runtime: Cell::new(false),
        runtime: RefCell::new(None),
        tunnel: RefCell::new(None),
        key: RefCell::new(None),
        challenge: RefCell::new(None),
    };
    let outcome = Pipeline {
        mode: EvidenceMode::Live,
        configuration: &journey,
        adb: &journey,
        mcp: &journey,
        tunnel: &journey,
        external: &journey,
    }
    .run(&ctx);
    let (code, status) = match outcome {
        PipelineOutcome::Ready { .. } => {
            eprintln!("Ready. Your selected conversation verified a read-only TV call. The controller remains running.");
            (0, ProgressOutcome::Verified)
        }
        PipelineOutcome::SimulationComplete { .. } => (2, ProgressOutcome::Simulated),
        PipelineOutcome::Stopped { stage, reason } => {
            journey.checkpoint.borrow_mut().stage = stage.into();
            let (outcome, event) = match reason {
                StopReason::Rejected => (ProgressOutcome::Rejected, EventCode::ApprovalRejected),
                StopReason::Cancelled => (ProgressOutcome::Cancelled, EventCode::Cancelled),
                StopReason::Expired => (ProgressOutcome::Expired, EventCode::Expired),
                StopReason::Pending => (ProgressOutcome::Pending, EventCode::AwaitingApproval),
                _ => (ProgressOutcome::Blocked, EventCode::Failed),
            };
            journey.store.borrow_mut().event(LogEvent {
                stage: stage.into(),
                code: event,
                elapsed_ms: clock.monotonic_ms(),
            })?;
            eprintln!("Setup paused at {stage:?}: {reason:?}. Run the same installer to resume.");
            if journey.runtime.borrow().is_none() && journey.tunnel.borrow().is_some() {
                eprintln!("A previously approved controller may still be running. Setup is not yet verified.");
            }
            (2, outcome)
        }
    };
    journey.checkpoint.borrow_mut().outcome = status;
    journey
        .store
        .borrow_mut()
        .save(&journey.checkpoint.borrow())?;
    eprintln!("Details: {}", journey.store.borrow().log_path().display());
    Ok(code)
}

pub fn bridge(path: &Path, state: &Path) -> Result<i32, &'static str> {
    let snapshot = private_read(path)?;
    let intent: RuntimeIntent = serde_json::from_slice(&private_read_limit(
        &state.join("runtime-intent.json"),
        6_400_000,
    )?)
    .map_err(|_| "runtime_intent_invalid")?;
    if intent.schema != 1
        || intent.configuration.as_bytes() != snapshot
        || !tv_domain::identifier(&intent.installation_id)
        || !tv_domain::identifier(&intent.device)
    {
        return Err("runtime_intent_mismatch");
    }
    let config = Arc::new(Config::from_json(&snapshot)?);
    let clock = SystemClock::default();
    let interrupts = Interrupts::install()?;
    let ctx = Context {
        clock: &clock,
        cancellation: &interrupts,
        deadline_monotonic_ms: 10_000,
    };
    let mut runtime = OwnedAdbRuntime::start(adb_spec(&config, &intent.device, state)?, true, &ctx)
        .map_err(|e| e.code)?;
    if runtime.connect(&ctx).map_err(|e| e.code)? != AdbAuthorization::Authorized {
        return Err("tv_authorization_pending");
    }
    super::mcp::serve_bridge(
        super::service(config.clone()),
        config,
        state.to_owned(),
        snapshot,
        &interrupts,
    )?;
    Ok(0)
}

/// Explicit lifecycle command; stops only the exact owned official alias and bridge.
pub fn disconnect(path: &Path, as_json: bool) -> Result<i32, &'static str> {
    let path = path
        .canonicalize()
        .map_err(|_| "configuration_unreadable")?;
    let state = path
        .parent()
        .ok_or("configuration_path_invalid")?
        .join("connection");
    let store = Store::open(&state)?;
    let checkpoint = store.load()?.ok_or("connection_progress_missing")?;
    let id = checkpoint.tunnel_id.ok_or("tunnel_configuration_missing")?;
    let intent: RuntimeIntent = serde_json::from_slice(&private_read_limit(
        &state.join("runtime-intent.json"),
        6_400_000,
    )?)
    .map_err(|_| "runtime_intent_invalid")?;
    let spec = TunnelSpec {
        binary: find_program("tunnel-client").ok_or("tunnel_client_missing")?,
        alias: format!("tv-{}", checkpoint.installation_id),
        tunnel_id: id,
        runtime_key_env: "TV_CONTROL_TUNNEL_KEY".into(),
        state_dir: state.join("tunnel"),
        mcp_binary: intent.controller_binary,
        configuration: path,
        bridge_state_dir: Some(state),
        executable_path: std::env::var_os("PATH").ok_or("executable_path_missing")?,
    };
    let clock = SystemClock::default();
    let interrupts = Interrupts::install()?;
    let ctx = Context {
        clock: &clock,
        cancellation: &interrupts,
        deadline_monotonic_ms: 10_000,
    };
    let process = BoundedProcess {
        max_output: 65536,
        poll_ms: 10,
    };
    let stopped = OfficialTunnel { process: &process }
        .stop(&spec, true, &ctx)
        .map_err(|e| e.code)?;
    if stopped.process_running {
        return Err("controller_stop_unverified");
    }
    if as_json {
        println!(
            "{}",
            json!({"status":"stopped","runtime_running":false,"credentials_removed":false})
        );
    } else {
        println!("Controller stopped. Private configuration and TV trust were preserved.");
    }
    Ok(0)
}
