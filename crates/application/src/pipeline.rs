//! Sequential onboarding policy. Adapters own interaction, persistence and subprocesses.
//! Resume reruns all observations; a persisted stage is progress text, never authorization.
//! Dependent ports receive a Context bounded by prerequisite validity. Adapters must
//! check that Context before effects and honor its deadline while waiting or running.
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
    /// Selects one execution mode for the entire run. Composition must supply
    /// adapters of that mode; simulated evidence cannot authorize a live stage.
    pub mode: EvidenceMode,
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
fn evidence<T>(mode: EvidenceMode, value: &Fresh<T>, ctx: &Context<'_>) -> Result<(), StopReason> {
    require(value.mode == mode, "evidence_mode_mismatch")?;
    fresh(value, ctx)
}
#[derive(Clone, Copy)]
struct Validity {
    observed_at_unix_ms: u64,
    expires_at_unix_ms: u64,
}
impl Validity {
    fn of<T>(evidence: &Fresh<T>) -> Self {
        Self {
            observed_at_unix_ms: evidence.observed_at_unix_ms,
            expires_at_unix_ms: evidence.expires_at_unix_ms,
        }
    }
    fn remaining_ms(self, now: u64) -> Result<u64, StopReason> {
        if self.observed_at_unix_ms > now || now >= self.expires_at_unix_ms {
            Err(StopReason::Blocked("evidence_not_fresh"))
        } else {
            Ok(self.expires_at_unix_ms - now)
        }
    }
}
fn dependent<T>(
    ctx: &Context<'_>,
    prerequisites: &[Validity],
    call: impl FnOnce(&Context<'_>) -> Gate<T>,
) -> Result<T, StopReason> {
    context_reason(ctx)?;
    // Sample monotonic first: conversion never assumes that Unix and monotonic
    // clocks share an origin, and time spent reading Unix cannot extend validity.
    let monotonic_now = ctx.clock.monotonic_ms();
    let unix_now = ctx.clock.unix_ms();
    let mut deadline = ctx.deadline_monotonic_ms;
    for validity in prerequisites {
        deadline = deadline.min(monotonic_now.saturating_add(validity.remaining_ms(unix_now)?));
    }
    let scoped = Context {
        clock: ctx.clock,
        cancellation: ctx.cancellation,
        deadline_monotonic_ms: deadline,
    };
    context_reason(ctx)?;
    if ctx.clock.monotonic_ms() >= deadline {
        return Err(StopReason::Blocked("evidence_not_fresh"));
    }
    let result = call(&scoped);
    // Parent cancellation/expiry retain their meaning; a prerequisite expiry is
    // a freshness failure even when the port returns Pending or fresh evidence.
    context_reason(ctx)?;
    for validity in prerequisites {
        validity.remaining_ms(ctx.clock.unix_ms())?;
    }
    if ctx.clock.monotonic_ms() >= deadline {
        return Err(StopReason::Blocked("evidence_not_fresh"));
    }
    match result {
        Gate::Observed(value) => Ok(value),
        Gate::Stopped(reason) => Err(reason),
    }
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
            evidence(self.mode, &adb_consent, ctx)?;
            require(
                adb_consent.value.configuration == config
                    && adb_consent.value.scope == ConsentScope::DedicatedAdb
                    && !adb_consent.value.approval_reference.is_empty(),
                "adb_consent_scope_mismatch",
            )?;
            let mut prerequisites = Vec::with_capacity(7);
            prerequisites.push(Validity::of(&adb_consent));
            stage = Stage::AdbRuntime;
            let runtime = dependent(ctx, &prerequisites, |step| {
                self.adb.owned_runtime(&adb_consent.value, step)
            })?;
            evidence(self.mode, &runtime, ctx)?;
            require(
                runtime.value.configuration == config
                    && !runtime.value.runtime_generation.is_empty(),
                "runtime_binding_mismatch",
            )?;
            prerequisites.push(Validity::of(&runtime));
            stage = Stage::DeviceObservation;
            let access = dependent(ctx, &prerequisites, |step| {
                self.adb.observe(&runtime.value, step)
            })?;
            evidence(self.mode, &access, ctx)?;
            require(
                access.value.binding == runtime.value && observed(&access.value.receipt),
                "read_only_access_unverified",
            )?;
            prerequisites.push(Validity::of(&access));
            stage = Stage::LocalMcp;
            let mcp = dependent(ctx, &prerequisites, |step| {
                self.mcp.initialize_and_list(&runtime.value, step)
            })?;
            evidence(self.mode, &mcp, ctx)?;
            require(
                mcp.value.binding == runtime.value
                    && mcp.value.initialized
                    && mcp.value.observe_tool_available,
                "local_mcp_unverified",
            )?;
            prerequisites.push(Validity::of(&mcp));
            stage = Stage::TunnelConsent;
            let tunnel_consent = dependent(ctx, &prerequisites, |step| {
                self.tunnel.consent(&config, step)
            })?;
            evidence(self.mode, &tunnel_consent, ctx)?;
            require(
                tunnel_consent.value.configuration == config
                    && tunnel_consent.value.scope == ConsentScope::TunnelAccess
                    && !tunnel_consent.value.approval_reference.is_empty(),
                "tunnel_consent_scope_mismatch",
            )?;
            prerequisites.push(Validity::of(&tunnel_consent));
            stage = Stage::TunnelRuntime;
            let tunnel = dependent(ctx, &prerequisites, |step| {
                self.tunnel
                    .readiness(&runtime.value, &tunnel_consent.value, step)
            })?;
            evidence(self.mode, &tunnel, ctx)?;
            require(
                tunnel.value.device == runtime.value && !tunnel.value.tunnel_generation.is_empty(),
                "tunnel_binding_mismatch",
            )?;
            prerequisites.push(Validity::of(&tunnel));
            stage = Stage::ExternalConversation;
            let challenge = dependent(ctx, &prerequisites, |step| {
                self.external.challenge(&tunnel.value, step)
            })?;
            evidence(self.mode, &challenge, ctx)?;
            require(
                challenge.value.binding == tunnel.value && !challenge.value.nonce.is_empty(),
                "challenge_binding_mismatch",
            )?;
            prerequisites.push(Validity::of(&challenge));
            let proof = dependent(ctx, &prerequisites, |step| {
                self.external.verify(&challenge.value, step)
            })?;
            // Recheck every prerequisite after potentially lengthy external approvals.
            for validity in [
                fresh(&adb_consent, ctx),
                fresh(&runtime, ctx),
                fresh(&access, ctx),
                fresh(&mcp, ctx),
                fresh(&tunnel_consent, ctx),
                fresh(&tunnel, ctx),
                fresh(&challenge, ctx),
                evidence(self.mode, &proof, ctx),
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
            Ok(if self.mode == EvidenceMode::Simulated {
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
