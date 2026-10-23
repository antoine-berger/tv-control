use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use tv_application::{pipeline::*, Cancellation, Clock, Context};
use tv_domain::{pipeline::*, Observed, Receipt, Status};

struct Time(AtomicU64);
impl Clock for Time {
    fn unix_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn monotonic_ms(&self) -> u64 {
        self.unix_ms()
    }
    fn sleep_ms(&self, duration: u64) {
        self.0.fetch_add(duration, Ordering::SeqCst);
    }
}
struct Cancel(bool);
impl Cancellation for Cancel {
    fn is_cancelled(&self) -> bool {
        self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    Pending,
    Rejected,
    WrongRuntime,
    WrongChallenge,
    Cached,
    Late,
    WrongConsent,
}
struct Fixture<'a> {
    time: &'a Time,
    mode: EvidenceMode,
    fault: Fault,
    observations: Cell<u32>,
}
impl Fixture<'_> {
    fn fresh<T>(&self, value: T) -> Fresh<T> {
        Fresh {
            value,
            observed_at_unix_ms: self.time.unix_ms(),
            expires_at_unix_ms: self.time.unix_ms() + 400,
            mode: self.mode,
        }
    }
    fn config(&self) -> Configuration {
        Configuration {
            revision: "revision-a".into(),
            device: "display-a".into(),
            target_conversation: "chosen-conversation".into(),
        }
    }
    fn consent(&self, scope: ConsentScope) -> Gate<Fresh<Consent>> {
        Gate::Observed(self.fresh(Consent {
            configuration: self.config(),
            scope,
            approval_reference: "explicit-approval-reference".into(),
        }))
    }
    fn receipt(&self, id: &str) -> Receipt {
        Receipt {
            request_id: id.into(),
            status: Status::Observed,
            code: "observed",
            observed: Some(Observed {
                connected: true,
                ..Observed::default()
            }),
            local_elapsed_ms: 1,
            cached: self.fault == Fault::Cached,
        }
    }
    fn run(&self, cancellation: &dyn Cancellation) -> PipelineOutcome {
        Pipeline {
            configuration: self,
            adb: self,
            mcp: self,
            tunnel: self,
            external: self,
        }
        .run(&Context {
            clock: self.time,
            cancellation,
            deadline_monotonic_ms: 1000,
        })
    }
}
impl ConfigurationPort for Fixture<'_> {
    fn configuration(&self, _: &Context<'_>) -> Gate<Configuration> {
        Gate::Observed(self.config())
    }
}
impl AdbSetupPort for Fixture<'_> {
    fn consent(&self, _: &Configuration, _: &Context<'_>) -> Gate<Fresh<Consent>> {
        if self.fault == Fault::Rejected {
            return Gate::Stopped(StopReason::Rejected);
        }
        self.consent(if self.fault == Fault::WrongConsent {
            ConsentScope::TunnelAccess
        } else {
            ConsentScope::DedicatedAdb
        })
    }
    fn owned_runtime(&self, grant: &Consent, _: &Context<'_>) -> Gate<Fresh<DeviceBinding>> {
        Gate::Observed(self.fresh(DeviceBinding {
            configuration: grant.configuration.clone(),
            runtime_generation: "owned-runtime-generation".into(),
        }))
    }
    fn observe(&self, binding: &DeviceBinding, _: &Context<'_>) -> Gate<Fresh<ReadOnlyAccess>> {
        self.observations.set(self.observations.get() + 1);
        let mut binding = binding.clone();
        if self.fault == Fault::WrongRuntime {
            binding.runtime_generation = "another-runtime".into();
        }
        Gate::Observed(self.fresh(ReadOnlyAccess {
            binding,
            receipt: self.receipt("local-observe"),
        }))
    }
}
impl LocalMcpPort for Fixture<'_> {
    fn initialize_and_list(
        &self,
        binding: &DeviceBinding,
        _: &Context<'_>,
    ) -> Gate<Fresh<McpReadiness>> {
        Gate::Observed(self.fresh(McpReadiness {
            binding: binding.clone(),
            initialized: true,
            observe_tool_available: true,
        }))
    }
}
impl TunnelSetupPort for Fixture<'_> {
    fn consent(&self, _: &Configuration, _: &Context<'_>) -> Gate<Fresh<Consent>> {
        self.consent(ConsentScope::TunnelAccess)
    }
    fn readiness(
        &self,
        binding: &DeviceBinding,
        _: &Consent,
        _: &Context<'_>,
    ) -> Gate<Fresh<ConnectionBinding>> {
        Gate::Observed(self.fresh(ConnectionBinding {
            device: binding.clone(),
            tunnel_generation: "tunnel-generation".into(),
        }))
    }
}
impl ExternalConversationPort for Fixture<'_> {
    fn challenge(
        &self,
        binding: &ConnectionBinding,
        _: &Context<'_>,
    ) -> Gate<Fresh<ProbeChallenge>> {
        Gate::Observed(self.fresh(ProbeChallenge {
            nonce: "fixture-only-nonce".into(),
            binding: binding.clone(),
        }))
    }
    fn verify(
        &self,
        challenge: &ProbeChallenge,
        _: &Context<'_>,
    ) -> Gate<Fresh<ExternalConversationVerification>> {
        if self.fault == Fault::Pending {
            return Gate::Stopped(StopReason::Pending);
        }
        if self.fault == Fault::Late {
            self.time.0.store(500, Ordering::SeqCst);
        }
        let mut challenge = challenge.clone();
        if self.fault == Fault::WrongChallenge {
            challenge.binding.tunnel_generation = "different-tunnel".into();
        }
        Gate::Observed(self.fresh(ExternalConversationVerification {
            receipt: self.receipt(&challenge.nonce),
            challenge,
            confirmation_reference: "fixture-external-confirmation".into(),
        }))
    }
}
fn fixture(time: &Time, fault: Fault) -> Fixture<'_> {
    Fixture {
        time,
        mode: EvidenceMode::Simulated,
        fault,
        observations: Cell::new(0),
    }
}
#[test]
fn complete_fixture_is_simulation_never_ready() {
    let time = Time(AtomicU64::new(100));
    assert!(matches!(
        fixture(&time, Fault::None).run(&Cancel(false)),
        PipelineOutcome::SimulationComplete { .. }
    ));
}
#[test]
fn healthy_local_mcp_and_tunnel_do_not_prove_target_conversation() {
    let time = Time(AtomicU64::new(100));
    assert_eq!(
        fixture(&time, Fault::Pending).run(&Cancel(false)),
        PipelineOutcome::Stopped {
            stage: Stage::ExternalConversation,
            reason: StopReason::Pending
        }
    );
}
#[test]
fn consent_and_resource_bindings_fail_closed() {
    let time = Time(AtomicU64::new(100));
    for (fault, stage, reason) in [
        (Fault::Rejected, Stage::AdbConsent, StopReason::Rejected),
        (
            Fault::WrongConsent,
            Stage::AdbConsent,
            StopReason::Blocked("adb_consent_scope_mismatch"),
        ),
        (
            Fault::WrongRuntime,
            Stage::DeviceObservation,
            StopReason::Blocked("read_only_access_unverified"),
        ),
        (
            Fault::WrongChallenge,
            Stage::ExternalConversation,
            StopReason::Blocked("external_conversation_unverified"),
        ),
        (
            Fault::Cached,
            Stage::DeviceObservation,
            StopReason::Blocked("read_only_access_unverified"),
        ),
    ] {
        assert_eq!(
            fixture(&time, fault).run(&Cancel(false)),
            PipelineOutcome::Stopped { stage, reason }
        );
    }
}
#[test]
fn external_approval_cannot_outlive_any_prerequisite() {
    let time = Time(AtomicU64::new(100));
    assert_eq!(
        fixture(&time, Fault::Late).run(&Cancel(false)),
        PipelineOutcome::Stopped {
            stage: Stage::ExternalConversation,
            reason: StopReason::Blocked("evidence_not_fresh")
        }
    );
}
#[test]
fn resume_reobserves_device_instead_of_trusting_previous_stage() {
    let time = Time(AtomicU64::new(100));
    let fixture = fixture(&time, Fault::Pending);
    fixture.run(&Cancel(false));
    fixture.run(&Cancel(false));
    assert_eq!(fixture.observations.get(), 2);
}
#[test]
fn cancellation_and_expiry_remain_distinct() {
    let time = Time(AtomicU64::new(100));
    assert_eq!(
        fixture(&time, Fault::None).run(&Cancel(true)),
        PipelineOutcome::Stopped {
            stage: Stage::Configuration,
            reason: StopReason::Cancelled
        }
    );
    time.0.store(1000, Ordering::SeqCst);
    assert_eq!(
        fixture(&time, Fault::None).run(&Cancel(false)),
        PipelineOutcome::Stopped {
            stage: Stage::Configuration,
            reason: StopReason::Expired
        }
    );
}
