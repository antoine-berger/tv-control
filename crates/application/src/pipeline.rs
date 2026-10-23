//! Sequential onboarding policy. Adapters own interaction, persistence and subprocesses.
//! Resume reruns all observations; a persisted stage is progress text, never authorization.
use crate::Context;
use tv_domain::{pipeline::*, Receipt, Status};

pub trait ConfigurationPort {
    fn configuration(&self, ctx: &Context<'_>) -> Gate<Configuration>;
}
pub trait AdbSetupPort {
    /// Ask for or verify explicit authorization before creating keys or an owned daemon.
    fn consent(&self, config: &Configuration, ctx: &Context<'_>) -> Gate<Fresh<Consent>>;
    fn owned_runtime(&self, consent: &Consent, ctx: &Context<'_>) -> Gate<Fresh<DeviceBinding>>;
    fn observe(&self, binding: &DeviceBinding, ctx: &Context<'_>) -> Gate<Fresh<ReadOnlyAccess>>;
}
pub trait LocalMcpPort {
    fn initialize_and_list(
        &self,
        binding: &DeviceBinding,
        ctx: &Context<'_>,
    ) -> Gate<Fresh<McpReadiness>>;
}
pub trait TunnelSetupPort {
    fn consent(&self, config: &Configuration, ctx: &Context<'_>) -> Gate<Fresh<Consent>>;
    fn readiness(
        &self,
        binding: &DeviceBinding,
        consent: &Consent,
        ctx: &Context<'_>,
    ) -> Gate<Fresh<ConnectionBinding>>;
}
pub trait ExternalConversationPort {
    /// Generate a new unpredictable nonce; never restore a saved challenge on resume.
    fn challenge(
        &self,
        binding: &ConnectionBinding,
        ctx: &Context<'_>,
    ) -> Gate<Fresh<ProbeChallenge>>;
    /// Guide the actual target conversation call and obtain explicit external confirmation.
    /// Implementations must return Pending if this cannot be established. Local traffic
    /// and user pressing Enter in this CLI cannot establish conversation exposure.
    fn verify(
        &self,
        challenge: &ProbeChallenge,
        ctx: &Context<'_>,
    ) -> Gate<Fresh<ExternalConversationVerification>>;
}

pub struct Pipeline<'a> {
    pub configuration: &'a dyn ConfigurationPort,
    pub adb: &'a dyn AdbSetupPort,
    pub mcp: &'a dyn LocalMcpPort,
    pub tunnel: &'a dyn TunnelSetupPort,
    pub external: &'a dyn ExternalConversationPort,
}

fn context_reason(ctx: &Context<'_>) -> Result<(), StopReason> {
    ctx.check().map_err(|failure| match failure.code {
        "cancelled" => StopReason::Cancelled,
        "expired" => StopReason::Expired,
        code => StopReason::Error(code),
    })
}
fn checked<T>(ctx: &Context<'_>, call: impl FnOnce() -> Gate<T>) -> Result<T, StopReason> {
    context_reason(ctx)?;
    let result = call();
    context_reason(ctx)?;
    match result {
        Gate::Observed(value) => Ok(value),
        Gate::Stopped(reason) => Err(reason),
    }
}
fn fresh<T>(evidence: &Fresh<T>, ctx: &Context<'_>) -> Result<(), StopReason> {
    let now = ctx.clock.unix_ms();
    if evidence.observed_at_unix_ms > now || now >= evidence.expires_at_unix_ms {
        return Err(StopReason::Blocked("evidence_not_fresh"));
    }
    Ok(())
}
fn require(valid: bool, code: &'static str) -> Result<(), StopReason> {
    if valid {
        Ok(())
    } else {
        Err(StopReason::Blocked(code))
    }
}
fn observed(receipt: &Receipt) -> bool {
    receipt.status == Status::Observed
        && !receipt.cached
        && receipt
            .observed
            .as_ref()
            .is_some_and(|state| state.connected)
}

impl Pipeline<'_> {
    pub fn run(&self, ctx: &Context<'_>) -> PipelineOutcome {
        let mut stage = Stage::Configuration;
        let result = (|| {
            let config = checked(ctx, || self.configuration.configuration(ctx))?;
            require(
                !config.revision.is_empty()
                    && !config.device.is_empty()
                    && !config.target_conversation.is_empty(),
                "configuration_incomplete",
            )?;
            stage = Stage::AdbConsent;
            let adb_consent = checked(ctx, || self.adb.consent(&config, ctx))?;
            fresh(&adb_consent, ctx)?;
            require(
                adb_consent.value.configuration == config
                    && adb_consent.value.scope == ConsentScope::DedicatedAdb
                    && !adb_consent.value.approval_reference.is_empty(),
                "adb_consent_scope_mismatch",
            )?;
            stage = Stage::AdbRuntime;
            let runtime = checked(ctx, || self.adb.owned_runtime(&adb_consent.value, ctx))?;
            fresh(&runtime, ctx)?;
            require(
                runtime.value.configuration == config
                    && !runtime.value.runtime_generation.is_empty(),
                "runtime_binding_mismatch",
            )?;
            stage = Stage::DeviceObservation;
            let access = checked(ctx, || self.adb.observe(&runtime.value, ctx))?;
            fresh(&access, ctx)?;
            require(
                access.value.binding == runtime.value && observed(&access.value.receipt),
                "read_only_access_unverified",
            )?;
            stage = Stage::LocalMcp;
            let mcp = checked(ctx, || self.mcp.initialize_and_list(&runtime.value, ctx))?;
            fresh(&mcp, ctx)?;
            require(
                mcp.value.binding == runtime.value
                    && mcp.value.initialized
                    && mcp.value.observe_tool_available,
                "local_mcp_unverified",
            )?;
            stage = Stage::TunnelConsent;
            let tunnel_consent = checked(ctx, || self.tunnel.consent(&config, ctx))?;
            fresh(&tunnel_consent, ctx)?;
            require(
                tunnel_consent.value.configuration == config
                    && tunnel_consent.value.scope == ConsentScope::TunnelAccess
                    && !tunnel_consent.value.approval_reference.is_empty(),
                "tunnel_consent_scope_mismatch",
            )?;
            stage = Stage::TunnelRuntime;
            let tunnel = checked(ctx, || {
                self.tunnel
                    .readiness(&runtime.value, &tunnel_consent.value, ctx)
            })?;
            fresh(&tunnel, ctx)?;
            require(
                tunnel.value.device == runtime.value && !tunnel.value.tunnel_generation.is_empty(),
                "tunnel_binding_mismatch",
            )?;
            stage = Stage::ExternalConversation;
            let challenge = checked(ctx, || self.external.challenge(&tunnel.value, ctx))?;
            fresh(&challenge, ctx)?;
            require(
                challenge.value.binding == tunnel.value && !challenge.value.nonce.is_empty(),
                "challenge_binding_mismatch",
            )?;
            let proof = checked(ctx, || self.external.verify(&challenge.value, ctx))?;
            // Recheck every prerequisite after potentially lengthy external approvals.
            for validity in [
                fresh(&adb_consent, ctx),
                fresh(&runtime, ctx),
                fresh(&access, ctx),
                fresh(&mcp, ctx),
                fresh(&tunnel_consent, ctx),
                fresh(&tunnel, ctx),
                fresh(&challenge, ctx),
                fresh(&proof, ctx),
            ] {
                validity?;
            }
            require(
                proof.value.challenge == challenge.value
                    && !proof.value.confirmation_reference.is_empty()
                    && proof.value.receipt.request_id == challenge.value.nonce
                    && observed(&proof.value.receipt)
                    && proof.observed_at_unix_ms >= challenge.observed_at_unix_ms,
                "external_conversation_unverified",
            )?;
            let simulated = [
                adb_consent.mode,
                runtime.mode,
                access.mode,
                mcp.mode,
                tunnel_consent.mode,
                tunnel.mode,
                challenge.mode,
                proof.mode,
            ]
            .contains(&EvidenceMode::Simulated);
            Ok(if simulated {
                PipelineOutcome::SimulationComplete {
                    binding: tunnel.value,
                }
            } else {
                PipelineOutcome::Ready {
                    binding: tunnel.value,
                    verified_at_unix_ms: ctx.clock.unix_ms(),
                }
            })
        })();
        result.unwrap_or_else(|reason| PipelineOutcome::Stopped { stage, reason })
    }
}
