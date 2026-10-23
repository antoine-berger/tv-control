//! Resume an exact owned connection without replaying first-install onboarding.
use super::{
    bounded, credential,
    runtime::{private_read_limit, resolve_tunnel, RuntimeIntent},
};
use crate::terminal::Terminal;
use std::{ffi::OsStr, path::Path};
use tv_adapters::{
    config::Config,
    connection_proof::ProofStore,
    onboarding_tunnel::{OfficialTunnel, TunnelSpec},
    pipeline_store::{Checkpoint, ProgressOutcome, ProgressStage, Store},
    process::BoundedProcess,
};
use tv_application::Context;

#[allow(clippy::too_many_arguments)]
pub(super) fn resume(
    path: &Path,
    state: &Path,
    snapshot: &[u8],
    config: &Config,
    device: &str,
    launcher: &Path,
    previous: Option<&Checkpoint>,
    terminal: &Terminal,
    ctx: &Context<'_>,
    startup_key: &mut Option<String>,
    store: &mut Store,
    verification_ready: &mut bool,
) -> Result<Option<i32>, &'static str> {
    let intent_path = state.join("runtime-intent.json");
    match intent_path.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("runtime_intent_unreadable"),
        Ok(_) => {}
    }
    let saved = previous.ok_or("runtime_intent_without_checkpoint")?;
    let intent: RuntimeIntent =
        serde_json::from_slice(&private_read_limit(&intent_path, 6_400_000)?)
            .map_err(|_| "runtime_intent_invalid")?;
    if saved.configuration_path != path
        || intent.schema != 1
        || intent.installation_id != saved.installation_id
        || intent.device != device
        || !tv_domain::identifier(&intent.revision)
    {
        return Err("runtime_intent_mismatch");
    }
    if intent.controller_binary != launcher {
        return Err("runtime_launcher_changed");
    }
    let id = saved
        .tunnel_id
        .as_deref()
        .ok_or("tunnel_configuration_missing")?;
    // Configuration changes require the normal explicit reapproval path, but
    // credentials must be available before that path may stop a controller.
    if intent.configuration.as_bytes() != snapshot || !config.generic_adb_control {
        if credential::available(state, &saved.installation_id, id)?.is_none() {
            eprintln!("This update needs a controller restart to apply new configuration/access. The previous release did not save its runtime key. Keeping your existing controller and configuration unchanged unless you choose to supply a replacement.");
            if terminal
                .line(
                    "Prepare a runtime key for this migration now? [y/N] ",
                    false,
                    ctx,
                )?
                .as_deref()
                != Some("y")
            {
                eprintln!("Release installed; controller migration deferred. No controller was stopped. The existing process, if running, still uses the previous code/access.");
                return Ok(Some(0));
            }
        }
        *startup_key = Some(credential::acquire(
            state,
            &saved.installation_id,
            id,
            terminal,
            ctx,
        )?);
        return Ok(None);
    }
    let spec = TunnelSpec {
        binary: resolve_tunnel(state, ctx)?.ok_or("tunnel_client_missing")?,
        alias: format!("tv-{}", saved.installation_id),
        tunnel_id: id.into(),
        runtime_key_env: "TV_CONTROL_TUNNEL_KEY".into(),
        state_dir: state.join("tunnel"),
        mcp_binary: launcher.into(),
        configuration: path.into(),
        bridge_state_dir: Some(state.into()),
        executable_path: std::env::var_os("PATH").ok_or("executable_path_missing")?,
    };
    let process = BoundedProcess {
        max_output: 65_536,
        poll_ms: 10,
    };
    let tunnel = OfficialTunnel { process: &process };
    let health = match tunnel.status(&spec, &bounded(ctx, 5_000)) {
        Ok(health) => Some(health),
        Err(error) if error.code == "tunnel_state_missing" => None,
        Err(error) => return Err(error.code),
    };
    eprintln!("To resume or repeat chat verification explicitly, use verify-connection with this configuration and the same --launcher path.");
    let healthy = health.as_ref().is_some_and(|health| health.locally_ready());
    let previous_generation = match tv_adapters::runtime_intent::read_generation(
        state,
        &saved.installation_id,
        &intent.revision,
        device,
    ) {
        Ok(generation) => Some(generation),
        Err("bridge_generation_unavailable" | "bridge_generation_binding_mismatch") if !healthy => {
            None
        }
        Err(code) => return Err(code),
    };
    // Recover an interrupted intent/checkpoint publication only after checking
    // the exact live binding. Never preserve verified status across revisions.
    if saved.configuration_revision != intent.revision {
        let mut reconciled = saved.clone();
        reconciled.configuration_revision = intent.revision.clone();
        reconciled.stage = ProgressStage::TunnelRuntime;
        reconciled.outcome = ProgressOutcome::Pending;
        store.save(&reconciled)?;
    }
    let mut prepared_key = None;
    if healthy {
        *verification_ready = true;
        tv_adapters::runtime_intent::read_generation(state, &saved.installation_id, &intent.revision, device)?;
        eprintln!("Existing controller is locally ready. Saved TV identity, tunnel and configuration are unchanged. No new conversation verification was performed.");
        if credential::available(state, &saved.installation_id, id)?.is_none() {
            eprintln!("No saved runtime key is available. Your running controller can be preserved without it. The installed update takes effect after a restart, which requires a runtime key.");
            if terminal.line("Configure a runtime key for future updates now? (optional) [y/N] ", false, ctx)?.as_deref() != Some("y") {
                eprintln!("Running controller preserved; installed code will be used on its next restart.");
                return Ok(Some(0));
            }
            prepared_key = Some(credential::acquire(state, &saved.installation_id, id, terminal, ctx)?);
        }
        if terminal.line("Restart this owned controller with the installed version and the same approved access? [y/N] ", false, ctx)?.as_deref() != Some("y") {
            eprintln!("Running controller preserved; installed code will be used on its next restart.");
            return Ok(Some(0));
        }
    } else if terminal.line("Resume this saved controller with the same TV identity, tunnel and approved access? [y/N] ", false, ctx)?.as_deref() != Some("y") {
        return Err("approval_rejected");
    }
    // Resolve before stop: no destructive lifecycle action while the only key
    // may still exist solely in the old process's environment.
    let key = match prepared_key {
        Some(key) => key,
        None => credential::acquire(state, &saved.installation_id, id, terminal, ctx)?,
    };
    if super::runtime::private_read(path)? != snapshot {
        return Err("configuration_changed_restart_setup");
    }
    if health.as_ref().is_some_and(|health| health.process_running) {
        let stopped = tunnel
            .stop(&spec, true, &bounded(ctx, 10_000))
            .map_err(|e| e.code)?;
        let checked = tunnel
            .status(&spec, &bounded(ctx, 5_000))
            .map_err(|e| e.code)?;
        if stopped.process_running || checked.process_running {
            return Err("controller_stop_unverified");
        }
    }
    ProofStore::open(&state.join("verification"))?.invalidate()?;
    let mut restarting = saved.clone();
    restarting.configuration_revision = intent.revision.clone();
    restarting.stage = ProgressStage::TunnelRuntime;
    restarting.outcome = ProgressOutcome::Pending;
    store.save(&restarting)?;
    let health = tunnel
        .ensure(&spec, true, Some(OsStr::new(&key)), &bounded(ctx, 60_000))
        .map_err(|e| e.code)?;
    if !health.locally_ready() {
        return Err("tunnel_not_ready");
    }
    let generation_ctx = bounded(ctx, 5_000);
    loop {
        generation_ctx.check().map_err(|e| e.code)?;
        match tv_adapters::runtime_intent::read_generation(
            state,
            &saved.installation_id,
            &intent.revision,
            device,
        ) {
            Ok(generation) if Some(&generation) != previous_generation.as_ref() => break,
            Ok(_)
            | Err(
                "bridge_generation_unavailable"
                | "bridge_generation_binding_mismatch"
                | "bridge_generation_pending",
            ) => generation_ctx.wait(20).map_err(|e| e.code)?,
            Err(code) => return Err(code),
        }
    }
    *verification_ready = true;
    eprintln!("Controller updated and locally ready. TV identity, configuration and tunnel were preserved. Conversation verification was not repeated; its previous proof was invalidated by the restart.");
    Ok(Some(0))
}
