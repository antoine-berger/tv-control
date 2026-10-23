//! Offline pipeline driver. No native adapter, subprocess, credential or terminal access.
use serde::Deserialize;
use serde_json::json;
use std::{cell::RefCell, fs::OpenOptions, io::Read, os::unix::fs::OpenOptionsExt, path::Path};
use tv_adapters::config::{Config, Device};
use tv_application::{pipeline::*, Cancellation, Clock, Context};
use tv_domain::{pipeline::*, Observed, Receipt, Status};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StageName {
    Configuration,
    AdbConsent,
    AdbRuntime,
    DeviceObservation,
    LocalMcp,
    TunnelConsent,
    TunnelRuntime,
    ExternalConversation,
}
impl StageName {
    fn stage(self) -> Stage {
        match self {
            Self::Configuration => Stage::Configuration,
            Self::AdbConsent => Stage::AdbConsent,
            Self::AdbRuntime => Stage::AdbRuntime,
            Self::DeviceObservation => Stage::DeviceObservation,
            Self::LocalMcp => Stage::LocalMcp,
            Self::TunnelConsent => Stage::TunnelConsent,
            Self::TunnelRuntime => Stage::TunnelRuntime,
            Self::ExternalConversation => Stage::ExternalConversation,
        }
    }
}
fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Configuration => "configuration",
        Stage::AdbConsent => "adb_consent",
        Stage::AdbRuntime => "adb_runtime",
        Stage::DeviceObservation => "device_observation",
        Stage::LocalMcp => "local_mcp",
        Stage::TunnelConsent => "tunnel_consent",
        Stage::TunnelRuntime => "tunnel_runtime",
        Stage::ExternalConversation => "external_conversation",
    }
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Reason {
    Pending,
    Rejected,
    Cancelled,
    Expired,
}
impl Reason {
    fn reason(self) -> StopReason {
        match self {
            Self::Pending => StopReason::Pending,
            Self::Rejected => StopReason::Rejected,
            Self::Cancelled => StopReason::Cancelled,
            Self::Expired => StopReason::Expired,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    schema: u32,
    stop_stage: Option<StageName>,
    reason: Option<Reason>,
}
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, &'static str> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "fixture_input_unreadable")?;
    if !file
        .metadata()
        .map_err(|_| "fixture_input_unreadable")?
        .is_file()
    {
        return Err("fixture_input_not_regular");
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "fixture_input_unreadable")?;
    if bytes.len() as u64 > limit {
        return Err("fixture_input_too_large");
    }
    Ok(bytes)
}
struct FakeClock;
impl Clock for FakeClock {
    fn unix_ms(&self) -> u64 {
        1000
    }
    fn monotonic_ms(&self) -> u64 {
        1
    }
    fn sleep_ms(&self, _: u64) {}
}
impl Cancellation for FakeClock {
    fn is_cancelled(&self) -> bool {
        false
    }
}
struct Driver {
    fixture: Fixture,
    configuration: Configuration,
    stages: RefCell<Vec<&'static str>>,
}
impl Driver {
    fn step<T>(&self, stage: Stage, value: T) -> Gate<T> {
        self.stages.borrow_mut().push(stage_name(stage));
        if self.fixture.stop_stage.map(StageName::stage) == Some(stage) {
            Gate::Stopped(self.fixture.reason.unwrap_or(Reason::Pending).reason())
        } else {
            Gate::Observed(value)
        }
    }
    fn fresh<T>(&self, value: T) -> Fresh<T> {
        Fresh {
            value,
            observed_at_unix_ms: 1000,
            expires_at_unix_ms: 2000,
            mode: EvidenceMode::Simulated,
        }
    }
    fn consent(&self, scope: ConsentScope) -> Fresh<Consent> {
        self.fresh(Consent {
            configuration: self.configuration.clone(),
            scope,
            approval_reference: "simulated-approval".into(),
        })
    }
    fn receipt(&self, id: &str) -> Receipt {
        Receipt {
            request_id: id.into(),
            status: Status::Observed,
            code: "simulated_observation",
            observed: Some(Observed {
                connected: true,
                ..Observed::default()
            }),
            local_elapsed_ms: 0,
            cached: false,
        }
    }
}
impl ConfigurationPort for Driver {
    fn configuration(&self, _: &Context<'_>) -> Gate<Configuration> {
        self.step(Stage::Configuration, self.configuration.clone())
    }
}
impl AdbSetupPort for Driver {
    fn consent(&self, _: &Configuration, _: &Context<'_>) -> Gate<Fresh<Consent>> {
        self.step(Stage::AdbConsent, self.consent(ConsentScope::DedicatedAdb))
    }
    fn owned_runtime(&self, _: &Consent, _: &Context<'_>) -> Gate<Fresh<DeviceBinding>> {
        self.step(
            Stage::AdbRuntime,
            self.fresh(DeviceBinding {
                configuration: self.configuration.clone(),
                runtime_generation: "simulated-runtime".into(),
            }),
        )
    }
    fn observe(&self, binding: &DeviceBinding, _: &Context<'_>) -> Gate<Fresh<ReadOnlyAccess>> {
        self.step(
            Stage::DeviceObservation,
            self.fresh(ReadOnlyAccess {
                binding: binding.clone(),
                receipt: self.receipt("simulated-local-observe"),
            }),
        )
    }
}
impl LocalMcpPort for Driver {
    fn initialize_and_list(
        &self,
        binding: &DeviceBinding,
        _: &Context<'_>,
    ) -> Gate<Fresh<McpReadiness>> {
        self.step(
            Stage::LocalMcp,
            self.fresh(McpReadiness {
                binding: binding.clone(),
                initialized: true,
                observe_tool_available: true,
            }),
        )
    }
}
impl TunnelSetupPort for Driver {
    fn consent(&self, _: &Configuration, _: &Context<'_>) -> Gate<Fresh<Consent>> {
        self.step(
            Stage::TunnelConsent,
            self.consent(ConsentScope::TunnelAccess),
        )
    }
    fn readiness(
        &self,
        binding: &DeviceBinding,
        _: &Consent,
        _: &Context<'_>,
    ) -> Gate<Fresh<ConnectionBinding>> {
        self.step(
            Stage::TunnelRuntime,
            self.fresh(ConnectionBinding {
                device: binding.clone(),
                tunnel_generation: "simulated-tunnel".into(),
            }),
        )
    }
}
impl ExternalConversationPort for Driver {
    fn challenge(
        &self,
        binding: &ConnectionBinding,
        _: &Context<'_>,
    ) -> Gate<Fresh<ProbeChallenge>> {
        Gate::Observed(self.fresh(ProbeChallenge {
            nonce: "simulated-probe".into(),
            binding: binding.clone(),
        }))
    }
    fn verify(
        &self,
        challenge: &ProbeChallenge,
        _: &Context<'_>,
    ) -> Gate<Fresh<ExternalConversationVerification>> {
        self.step(
            Stage::ExternalConversation,
            self.fresh(ExternalConversationVerification {
                challenge: challenge.clone(),
                confirmation_reference: "simulated-conversation-selection".into(),
                receipt: self.receipt(&challenge.nonce),
            }),
        )
    }
}
pub fn run(configuration_path: &Path, fixture_path: &Path) -> Result<i32, &'static str> {
    let fixture: Fixture = serde_json::from_slice(&read_bounded(fixture_path, 16384)?)
        .map_err(|_| "invalid_journey_fixture")?;
    if fixture.schema != 1 || (fixture.stop_stage.is_none() && fixture.reason.is_some()) {
        return Err("invalid_journey_fixture");
    }
    let config = Config::from_json(&read_bounded(configuration_path, 1_048_576)?)?;
    let device = config
        .devices
        .iter()
        .find(|(_, device)| matches!(device, Device::Adb { .. }))
        .map(|(alias, _)| alias.clone())
        .ok_or("fixture_requires_adb_configuration")?;
    let driver = Driver {
        fixture,
        configuration: Configuration {
            revision: "simulated-configuration".into(),
            device,
            target_conversation: "simulated-conversation".into(),
        },
        stages: RefCell::new(Vec::new()),
    };
    let clock = FakeClock;
    let outcome = Pipeline {
        mode: EvidenceMode::Simulated,
        configuration: &driver,
        adb: &driver,
        mcp: &driver,
        tunnel: &driver,
        external: &driver,
    }
    .run(&Context {
        clock: &clock,
        cancellation: &clock,
        deadline_monotonic_ms: 1000,
    });
    let (value, exit) = match outcome {
        PipelineOutcome::SimulationComplete { .. } => (
            json!({"status":"simulation_complete","mode":"simulated","ready":false,"stages":driver.stages.borrow().as_slice()}),
            0,
        ),
        PipelineOutcome::Stopped { stage, reason } => {
            let reason = match reason {
                StopReason::Pending => "pending",
                StopReason::Rejected => "rejected",
                StopReason::Cancelled => "cancelled",
                StopReason::Expired => "expired",
                _ => return Err("journey_fixture_invariant_failed"),
            };
            (
                json!({"status":"paused","mode":"simulated","ready":false,"stage":stage_name(stage),"reason":reason,"stages":driver.stages.borrow().as_slice()}),
                2,
            )
        }
        PipelineOutcome::Ready { .. } => return Err("journey_fixture_live_outcome_forbidden"),
    };
    println!("{value}");
    Ok(exit)
}
