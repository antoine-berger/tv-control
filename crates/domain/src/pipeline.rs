//! Onboarding evidence. These values contain references and observations, never credentials.
use crate::Receipt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Configuration,
    AdbConsent,
    AdbRuntime,
    DeviceObservation,
    LocalMcp,
    TunnelConsent,
    TunnelRuntime,
    ExternalConversation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    Pending,
    Blocked(&'static str),
    Rejected,
    Cancelled,
    Expired,
    Error(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate<T> {
    Observed(T),
    Stopped(StopReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceMode {
    Live,
    Simulated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fresh<T> {
    pub value: T,
    pub observed_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub mode: EvidenceMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Configuration {
    pub revision: String,
    pub device: String,
    /// A user-selected target reference; not inferred from MCP clientInfo.
    pub target_conversation: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceBinding {
    pub configuration: Configuration,
    /// Identifies a dedicated, owned runtime generation, not a global ADB daemon.
    pub runtime_generation: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionBinding {
    pub device: DeviceBinding,
    pub tunnel_generation: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentScope {
    DedicatedAdb,
    TunnelAccess,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Consent {
    pub configuration: Configuration,
    pub scope: ConsentScope,
    pub approval_reference: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadOnlyAccess {
    pub binding: DeviceBinding,
    pub receipt: Receipt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpReadiness {
    pub binding: DeviceBinding,
    pub initialized: bool,
    pub observe_tool_available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeChallenge {
    pub nonce: String,
    pub binding: ConnectionBinding,
}

/// Supplied by an explicit external verification workflow in the intended conversation.
/// A local handshake, clientInfo, health endpoint or nonce arrival alone is insufficient.
/// No cryptographic conversation identity is implied by this type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalConversationVerification {
    pub challenge: ProbeChallenge,
    pub confirmation_reference: String,
    /// The actual harmless Observe call, made through the target conversation's plugin.
    pub receipt: Receipt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineOutcome {
    Stopped {
        stage: Stage,
        reason: StopReason,
    },
    Ready {
        binding: ConnectionBinding,
        verified_at_unix_ms: u64,
    },
    SimulationComplete {
        binding: ConnectionBinding,
    },
}
