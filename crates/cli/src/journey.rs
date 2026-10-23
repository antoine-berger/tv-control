//! Composition of the guided journey. Account approvals remain explicit and resumable.
mod conversation;
mod local;
mod runtime;
mod tunnel;

use crate::terminal::{Interrupts, Terminal};
pub use runtime::{bridge, disconnect};
use runtime::{private_read, reconcile_runtime_intent};
use serde_json::json;
use std::os::unix::fs::MetadataExt;
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    sync::Arc,
};
use tv_adapters::{
    config::{Config, Device},
    connection_proof::{random_token, Challenge, ProofStore},
    onboarding_runtime::OwnedAdbRuntime,
    onboarding_tunnel::TunnelSpec,
    pipeline_store::{Checkpoint, EventCode, LogEvent, ProgressOutcome, ProgressStage, Store},
    runtime::SystemClock,
};
use tv_application::{pipeline::Pipeline, Clock, Context};
use tv_domain::{
    pipeline::{
        Configuration, Consent, ConsentScope, EvidenceMode, Fresh, Gate, PipelineOutcome, Stage,
        StopReason,
    },
    Action, Receipt, Request, Status,
};

const EVIDENCE_MS: u64 = 900_000;
const JOURNEY_MS: u64 = 1_800_000;
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
    fn bridge_generation(&self) -> Result<String, &'static str> {
        tv_adapters::runtime_intent::read_generation(
            &self.state,
            &self.installation,
            &self.configuration.revision,
            &self.configuration.device,
        )
    }
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
                expires_at_unix_ms: ctx.clock.unix_ms().saturating_add(
                    ctx.remaining_ms()
                        .min(self.config.preferences.max_budget_ms),
                ),
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
    let revision = reconcile_runtime_intent(
        &path,
        &state,
        &snapshot,
        &device,
        &controller_binary,
        previous.as_ref(),
        &terminal,
        &ctx,
    )?;
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
