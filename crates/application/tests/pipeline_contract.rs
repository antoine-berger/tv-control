use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use tv_application::{pipeline::*, Cancellation, Clock, Context};
use tv_domain::{pipeline::*, Observed, Receipt, Status};

struct Time(AtomicU64, u64);
impl Clock for Time {
    fn unix_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn monotonic_ms(&self) -> u64 {
        self.unix_ms().saturating_sub(self.1)
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
    SlowRuntime,
    SlowTunnelConsent,
    SlowTunnelRuntime,
    SlowChallenge,
    ParentExpiredRuntime,
    SimulatedConsent,
    SimulatedMcp,
}
struct Fixture<'a> {
    time: &'a Time,
    mode: EvidenceMode,
    fault: Fault,
    observations: Cell<u32>,
    calls: RefCell<Vec<(Stage, u64)>>,
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
        let mut consent = self.fresh(Consent {
            configuration: self.config(),
            scope,
            approval_reference: "explicit-approval-reference".into(),
        });
        if self.fault == Fault::SimulatedConsent {
            consent.mode = EvidenceMode::Simulated;
        }
        Gate::Observed(consent)
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
        self.run_mode(cancellation, self.mode)
    }
    fn run_mode(&self, cancellation: &dyn Cancellation, mode: EvidenceMode) -> PipelineOutcome {
        Pipeline {
            mode,
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
    fn owned_runtime(&self, grant: &Consent, ctx: &Context<'_>) -> Gate<Fresh<DeviceBinding>> {
        self.calls
            .borrow_mut()
            .push((Stage::AdbRuntime, ctx.deadline_monotonic_ms));
        if self.fault == Fault::SlowRuntime {
            self.time.sleep_ms(401);
        }
        if self.fault == Fault::ParentExpiredRuntime {
            self.time.sleep_ms(900);
        }
        Gate::Observed(self.fresh(DeviceBinding {
            configuration: grant.configuration.clone(),
            runtime_generation: "owned-runtime-generation".into(),
        }))
    }
    fn observe(&self, binding: &DeviceBinding, ctx: &Context<'_>) -> Gate<Fresh<ReadOnlyAccess>> {
        self.calls
            .borrow_mut()
            .push((Stage::DeviceObservation, ctx.deadline_monotonic_ms));
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
        ctx: &Context<'_>,
    ) -> Gate<Fresh<McpReadiness>> {
        self.calls
            .borrow_mut()
            .push((Stage::LocalMcp, ctx.deadline_monotonic_ms));
        let mut readiness = self.fresh(McpReadiness {
            binding: binding.clone(),
            initialized: true,
            observe_tool_available: true,
        });
        if self.fault == Fault::SimulatedMcp {
            readiness.mode = EvidenceMode::Simulated;
        }
        Gate::Observed(readiness)
    }
}
impl TunnelSetupPort for Fixture<'_> {
    fn consent(&self, _: &Configuration, ctx: &Context<'_>) -> Gate<Fresh<Consent>> {
        self.calls
            .borrow_mut()
            .push((Stage::TunnelConsent, ctx.deadline_monotonic_ms));
        if self.fault == Fault::SlowTunnelConsent {
            self.time.sleep_ms(401);
        }
        self.consent(ConsentScope::TunnelAccess)
    }
    fn readiness(
        &self,
        binding: &DeviceBinding,
        _: &Consent,
        ctx: &Context<'_>,
    ) -> Gate<Fresh<ConnectionBinding>> {
        self.calls
            .borrow_mut()
            .push((Stage::TunnelRuntime, ctx.deadline_monotonic_ms));
        if self.fault == Fault::SlowTunnelRuntime {
            self.time.sleep_ms(401);
        }
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
        ctx: &Context<'_>,
    ) -> Gate<Fresh<ProbeChallenge>> {
        self.calls
            .borrow_mut()
            .push((Stage::ExternalConversation, ctx.deadline_monotonic_ms));
        if self.fault == Fault::SlowChallenge {
            self.time.sleep_ms(401);
        }
        Gate::Observed(self.fresh(ProbeChallenge {
            nonce: "fixture-only-nonce".into(),
            binding: binding.clone(),
        }))
    }
    fn verify(
        &self,
        challenge: &ProbeChallenge,
        ctx: &Context<'_>,
    ) -> Gate<Fresh<ExternalConversationVerification>> {
        self.calls
            .borrow_mut()
            .push((Stage::ExternalConversation, ctx.deadline_monotonic_ms));
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
        calls: RefCell::new(Vec::new()),
    }
}
#[test]
fn complete_fixture_is_simulation_never_ready() {
    let time = Time(AtomicU64::new(100), 0);
    assert!(matches!(
        fixture(&time, Fault::None).run(&Cancel(false)),
        PipelineOutcome::SimulationComplete { .. }
    ));
}
#[test]
fn healthy_local_mcp_and_tunnel_do_not_prove_target_conversation() {
    let time = Time(AtomicU64::new(100), 0);
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
    let time = Time(AtomicU64::new(100), 0);
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
    let time = Time(AtomicU64::new(100), 0);
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
    let time = Time(AtomicU64::new(100), 0);
    let fixture = fixture(&time, Fault::Pending);
    fixture.run(&Cancel(false));
    fixture.run(&Cancel(false));
    assert_eq!(fixture.observations.get(), 2);
}
#[test]
fn cancellation_and_expiry_remain_distinct() {
    let time = Time(AtomicU64::new(100), 0);
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

#[test]
fn fresh_consent_after_slow_approval_does_not_run_dependent_steps() {
    let time = Time(AtomicU64::new(100), 0);
    let fixture = fixture(&time, Fault::SlowTunnelConsent);
    assert_eq!(
        fixture.run(&Cancel(false)),
        PipelineOutcome::Stopped {
            stage: Stage::TunnelConsent,
            reason: StopReason::Blocked("evidence_not_fresh"),
        }
    );
    assert_eq!(
        fixture.calls.borrow().last().unwrap().0,
        Stage::TunnelConsent
    );
    assert!(!fixture
        .calls
        .borrow()
        .iter()
        .any(|(stage, _)| matches!(stage, Stage::TunnelRuntime | Stage::ExternalConversation)));
}

#[test]
fn every_dependent_stage_stops_before_following_stage_if_prerequisites_expire() {
    for (fault, last) in [
        (Fault::SlowRuntime, Stage::AdbRuntime),
        (Fault::SlowTunnelRuntime, Stage::TunnelRuntime),
        (Fault::SlowChallenge, Stage::ExternalConversation),
    ] {
        let time = Time(AtomicU64::new(100), 0);
        let fixture = fixture(&time, fault);
        assert_eq!(
            fixture.run(&Cancel(false)),
            PipelineOutcome::Stopped {
                stage: last,
                reason: StopReason::Blocked("evidence_not_fresh"),
            }
        );
        assert_eq!(fixture.calls.borrow().last().unwrap().0, last);
        assert_eq!(
            fixture
                .calls
                .borrow()
                .iter()
                .filter(|(stage, _)| *stage == last)
                .count(),
            1
        );
    }
}

#[test]
fn dependent_context_converts_unix_expiry_without_changing_parent_deadline() {
    let offset = 1_700_000_000_000;
    let time = Time(AtomicU64::new(offset + 100), offset);
    let fixture = fixture(&time, Fault::None);
    assert!(matches!(
        fixture.run(&Cancel(false)),
        PipelineOutcome::SimulationComplete { .. }
    ));
    // Evidence expires at Unix offset+500; ports must receive monotonic 500,
    // not Unix expiry or the unchanged global monotonic deadline of 1000.
    assert_eq!(fixture.calls.borrow().len(), 7);
    assert!(fixture
        .calls
        .borrow()
        .iter()
        .all(|(_, deadline)| *deadline == 500));
    assert_eq!(time.monotonic_ms(), 100);
}

#[test]
fn all_live_evidence_is_required_for_ready() {
    let time = Time(AtomicU64::new(100), 0);
    let mut fixture = fixture(&time, Fault::None);
    // Synthetic Live-classified evidence exercises policy only, not real access.
    fixture.mode = EvidenceMode::Live;
    assert!(matches!(
        fixture.run(&Cancel(false)),
        PipelineOutcome::Ready { .. }
    ));
}

#[test]
fn simulated_consent_cannot_authorize_owned_runtime_in_live_mode() {
    let time = Time(AtomicU64::new(100), 0);
    let mut fixture = fixture(&time, Fault::SimulatedConsent);
    fixture.mode = EvidenceMode::Live;
    assert_eq!(
        fixture.run(&Cancel(false)),
        PipelineOutcome::Stopped {
            stage: Stage::AdbConsent,
            reason: StopReason::Blocked("evidence_mode_mismatch"),
        }
    );
    assert!(fixture.calls.borrow().is_empty());
}

#[test]
fn mixed_evidence_stops_before_any_following_stage_and_is_never_ready() {
    let time = Time(AtomicU64::new(100), 0);
    let mut fixture = fixture(&time, Fault::SimulatedMcp);
    fixture.mode = EvidenceMode::Live;
    assert_eq!(
        fixture.run(&Cancel(false)),
        PipelineOutcome::Stopped {
            stage: Stage::LocalMcp,
            reason: StopReason::Blocked("evidence_mode_mismatch"),
        }
    );
    assert_eq!(fixture.calls.borrow().last().unwrap().0, Stage::LocalMcp);
    let fixture = self::fixture(&time, Fault::None);
    assert_eq!(
        fixture.run_mode(&Cancel(false), EvidenceMode::Live),
        PipelineOutcome::Stopped {
            stage: Stage::AdbConsent,
            reason: StopReason::Blocked("evidence_mode_mismatch"),
        }
    );
    assert!(fixture.calls.borrow().is_empty());
}

#[test]
fn parent_cancellation_or_deadline_during_a_port_prevents_next_stage() {
    struct CancelAt<'a>(&'a Time);
    impl Cancellation for CancelAt<'_> {
        fn is_cancelled(&self) -> bool {
            self.0.unix_ms() >= 101
        }
    }
    let time = Time(AtomicU64::new(100), 0);
    let fixture = fixture(&time, Fault::SlowRuntime);
    assert_eq!(
        fixture.run(&CancelAt(&time)),
        PipelineOutcome::Stopped {
            stage: Stage::AdbRuntime,
            reason: StopReason::Cancelled,
        }
    );
    assert_eq!(fixture.observations.get(), 0);
    let time = Time(AtomicU64::new(100), 0);
    let fixture = self::fixture(&time, Fault::ParentExpiredRuntime);
    assert_eq!(
        fixture.run(&Cancel(false)),
        PipelineOutcome::Stopped {
            stage: Stage::AdbRuntime,
            reason: StopReason::Expired,
        }
    );
    assert_eq!(fixture.observations.get(), 0);
}
