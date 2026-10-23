//! Cooperative conversation verification bound to the live controller generation.
use super::runtime::private_read;
use super::{bounded, gate, guidance, Journey};
use tv_adapters::{
    connection_proof::{random_token, ProofStore},
    onboarding_tunnel::OfficialTunnel,
    process::BoundedProcess,
};
use tv_application::{pipeline::ExternalConversationPort, Context};
use tv_domain::pipeline::{
    ConnectionBinding, ExternalConversationVerification, Fresh, Gate, ProbeChallenge, Stage,
};

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
            guidance::plugin(&id);
            self.ask(
                "Press Enter when the plugin is selected, or q to resume later: ",
                false,
                ctx,
            )?;
            // The first plugin discovery happens while the user is in the UI.
            // Recheck the same owned runtime now: readyz alone can remain healthy
            // even when that child's MCP initialization or catalogue has failed.
            let process = BoundedProcess {
                max_output: 65_536,
                poll_ms: 10,
            };
            let tunnel = self.tunnel.borrow();
            let tunnel = tunnel.as_ref().ok_or("tunnel_configuration_missing")?;
            let health = (OfficialTunnel { process: &process })
                .status(tunnel, &bounded(ctx, 5_000))
                .map_err(|e| e.code)?;
            if !health.locally_ready() {
                return Err(health
                    .mcp_failure_code
                    .unwrap_or("tunnel_mcp_setup_not_ready"));
            }
            let challenge = ProofStore::open(&self.state.join("verification"))?.issue(
                &self.installation,
                &self.configuration.revision,
                &self.snapshot,
                &self.configuration.device,
                &binding.tunnel_generation,
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
            if self.bridge_generation()? != challenge.binding.tunnel_generation {
                return Err("controller_restarted_repeat_verification");
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
            if self.bridge_generation()? != challenge.binding.tunnel_generation {
                return Err("controller_restarted_repeat_verification");
            }
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
