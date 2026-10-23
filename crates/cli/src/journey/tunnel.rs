//! Explicit tunnel consent, dependency acquisition, and readiness port.
use super::runtime::{find_program, resolve_tunnel, RuntimeIntent};
use super::{bounded, gate, guidance, Journey};
use std::{ffi::OsStr, path::PathBuf};
use tv_adapters::{
    onboarding_tunnel::OfficialTunnel,
    process::BoundedProcess,
    tunnel_dependency::{self, Release},
};
use tv_application::{pipeline::TunnelSetupPort, Context};
use tv_domain::pipeline::{
    Configuration, ConnectionBinding, Consent, ConsentScope, DeviceBinding, Fresh, Gate, Stage,
};

impl TunnelSetupPort for Journey<'_> {
    fn consent(&self, config: &Configuration, ctx: &Context<'_>) -> Gate<Fresh<Consent>> {
        gate((|| {
            self.progress(Stage::TunnelConsent, "[3/4] ChatGPT connection", ctx)?;
            let binary = match resolve_tunnel(&self.state, ctx)? {
                Some(binary) => binary,
                None => match std::env::consts::OS {
                    "linux" => {
                        if self.ask("Install verified OpenAI tunnel-client v0.0.17 privately for this connection? [y/N] ", false, ctx)? != "y" {
                                return Err("approval_rejected");
                            }
                        eprintln!("Installing connection helper…");
                        let process = BoundedProcess {
                            max_output: 64 * 1024 * 1024,
                            poll_ms: 20,
                        };
                        tunnel_dependency::install_linux(
                            &self.state,
                            Release::linux(std::env::consts::ARCH)?,
                            true,
                            &process,
                            &bounded(ctx, 180_000),
                        )?
                    }
                    "macos" => {
                        let brew = find_program("brew")
                            .or_else(|| {
                                ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
                                    .into_iter()
                                    .map(PathBuf::from)
                                    .find(|p| p.is_file())
                            })
                            .ok_or_else(|| {
                                eprintln!("Homebrew is required for the supported macOS helper installation. Install it from https://brew.sh, then rerun this command. Progress is saved.");
                                "tunnel_dependency_homebrew_required"
                            })?;
                        if self.ask("Install OpenAI tunnel-client using Homebrew (current official formula and dependencies, in Homebrew's prefix)? [y/N] ", false, ctx)? != "y" {
                                return Err("approval_rejected");
                            }
                        eprintln!("Installing connection helper…");
                        let process = BoundedProcess {
                            max_output: 1024 * 1024,
                            poll_ms: 50,
                        };
                        tunnel_dependency::install_homebrew(
                            &brew,
                            true,
                            &process,
                            &bounded(ctx, 600_000),
                        )?;
                        resolve_tunnel(&self.state, ctx)?
                            .ok_or("tunnel_dependency_homebrew_not_on_path")?
                    }
                    _ => return Err("tunnel_dependency_platform_unsupported"),
                },
            };
            let saved = self.checkpoint.borrow().tunnel_id.clone();
            let id = match saved {
                Some(id) => id,
                None => {
                    guidance::tunnel_setup();
                    loop {
                        let id = self.ask("Tunnel ID: ", false, ctx)?;
                        if id == "help" {
                            guidance::permissions();
                        } else {
                            break id;
                        }
                    }
                }
            };
            if !self.tunnel_approved.get() {
                if self.ask("Allow a background TV controller through this tunnel until you stop it? [y/N] ",false,ctx)? != "y" {return Err("approval_rejected");}
                self.tunnel_approved.set(true);
            }
            if !self.existing_runtime.get() && self.key.borrow().is_none() {
                *self.key.borrow_mut() = Some(self.acquire_key(&id, ctx)?);
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
            let previous_generation = if self.existing_runtime.get() {
                None
            } else {
                match self.bridge_generation() {
                    Ok(generation) => Some(generation),
                    Err("bridge_generation_unavailable" | "bridge_generation_binding_mismatch") => {
                        None
                    }
                    Err(code) => return Err(code),
                }
            };
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
            let generation_ctx = bounded(ctx, 5_000);
            let generation = loop {
                generation_ctx.check().map_err(|e| e.code)?;
                match self.bridge_generation() {
                    Ok(generation)
                        if self.existing_runtime.get()
                            || previous_generation.as_ref() != Some(&generation) =>
                    {
                        break generation
                    }
                    Ok(_)
                    | Err(
                        "bridge_generation_unavailable"
                        | "bridge_generation_binding_mismatch"
                        | "bridge_generation_pending",
                    ) => {
                        generation_ctx.wait(20).map_err(|e| e.code)?;
                    }
                    Err(code) => return Err(code),
                }
            };
            Ok(self.fresh(
                ConnectionBinding {
                    device: binding.clone(),
                    tunnel_generation: generation,
                },
                ctx,
            ))
        })())
    }
}
