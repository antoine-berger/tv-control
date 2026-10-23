//! Owned runtime recovery, bridge execution, and disconnect lifecycle.
use super::bounded;
use crate::terminal::{Interrupts, Terminal};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use tv_adapters::{
    config::{Config, Device},
    connection_proof::{random_token, ProofStore},
    onboarding_runtime::{AdbAuthorization, AdbRuntimeSpec, OwnedAdbRuntime},
    onboarding_tunnel::{OfficialTunnel, TunnelSpec},
    pipeline_store::{Checkpoint, Store},
    process::BoundedProcess,
    runtime::SystemClock,
    tunnel_dependency::{self, Release},
};
use tv_application::Context;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeIntent {
    pub(super) schema: u32,
    pub(super) installation_id: String,
    pub(super) revision: String,
    pub(super) device: String,
    pub(super) configuration: String,
    pub(super) controller_binary: PathBuf,
}
pub(super) fn private_read(path: &Path) -> Result<Vec<u8>, &'static str> {
    private_read_limit(path, 1_048_576)
}
pub(super) fn private_read_limit(path: &Path, limit: usize) -> Result<Vec<u8>, &'static str> {
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "runtime_intent_unreadable")?;
    let m = f.metadata().map_err(|_| "runtime_intent_unreadable")?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.nlink() != 1
        || m.mode() & 0o077 != 0
    {
        return Err("runtime_intent_not_private");
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut f)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "runtime_intent_unreadable")?;
    if bytes.len() > limit {
        return Err("runtime_intent_too_large");
    }
    Ok(bytes)
}
pub(super) fn adb_spec(
    cfg: &Config,
    alias: &str,
    state: &Path,
) -> Result<AdbRuntimeSpec, &'static str> {
    match cfg.devices.get(alias) {
        Some(Device::Adb {
            endpoint,
            server_host,
            server_port,
            ..
        }) if server_host == "127.0.0.1" => Ok(AdbRuntimeSpec {
            executable: cfg.executables.adb.clone(),
            state_dir: state.join("adb"),
            port: *server_port,
            endpoint: endpoint.clone(),
            poll_ms: cfg.preferences.process_poll_ms,
        }),
        _ => Err("onboarding_requires_dedicated_adb_device"),
    }
}
pub(super) fn verify_adb_dependency(
    cfg: &Config,
    state: &Path,
    ctx: &Context<'_>,
) -> Result<(), &'static str> {
    let parent = state.parent().ok_or("configuration_path_invalid")?;
    let process = BoundedProcess {
        max_output: 65_536,
        poll_ms: 10,
    };
    tv_adapters::dependency_acquisition::verify_adb_path(
        &cfg.executables.adb,
        parent,
        &process,
        &bounded(ctx, 10_000),
    )
}
pub(super) fn find_program(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|p| p.is_absolute())
        .map(|p| p.join(name))
        .find_map(|p| {
            let m = fs::metadata(&p).ok()?;
            (m.is_file() && m.permissions().mode() & 0o111 != 0)
                .then(|| p.canonicalize().ok())
                .flatten()
        })
}
// Lifecycle operations resolve the same private dependency as initial acquisition.
pub(super) fn resolve_tunnel(
    state: &Path,
    ctx: &Context<'_>,
) -> Result<Option<PathBuf>, &'static str> {
    if std::env::consts::OS == "linux" {
        let process = BoundedProcess {
            max_output: 65_536,
            poll_ms: 10,
        };
        if let Some(binary) = tunnel_dependency::installed(
            state,
            Release::linux(std::env::consts::ARCH)?,
            &process,
            ctx,
        )? {
            return Ok(Some(binary));
        }
    }
    Ok(find_program("tunnel-client").or_else(|| {
        if std::env::consts::OS != "macos" {
            return None;
        }
        [
            "/opt/homebrew/bin/tunnel-client",
            "/usr/local/bin/tunnel-client",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find_map(|p| {
            let m = fs::metadata(&p).ok()?;
            (m.is_file() && m.permissions().mode() & 0o111 != 0)
                .then(|| p.canonicalize().ok())
                .flatten()
        })
    }))
}
#[allow(clippy::too_many_arguments)]
pub(super) fn reconcile_runtime_intent(
    path: &Path,
    state: &Path,
    snapshot: &[u8],
    device: &str,
    controller_binary: &Path,
    previous: Option<&Checkpoint>,
    terminal: &Terminal,
    ctx: &Context<'_>,
) -> Result<String, &'static str> {
    if previous.is_some_and(|saved| saved.configuration_path != path) {
        return Err("runtime_configuration_path_changed");
    }
    let intent_path = state.join("runtime-intent.json");
    let bytes = match intent_path.symlink_metadata() {
        Ok(_) => private_read_limit(&intent_path, 6_400_000)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return random_token(),
        Err(_) => return Err("runtime_intent_unreadable"),
    };
    let saved = previous.ok_or("runtime_intent_without_checkpoint")?;
    let intent: RuntimeIntent =
        serde_json::from_slice(&bytes).map_err(|_| "runtime_intent_invalid")?;
    if intent.schema != 1
        || intent.installation_id != saved.installation_id
        || !tv_domain::identifier(&intent.revision)
        || !tv_domain::identifier(&intent.device)
    {
        return Err("runtime_intent_mismatch");
    }
    if intent.controller_binary != controller_binary {
        return Err("runtime_launcher_changed");
    }
    let old_config = Config::from_json(intent.configuration.as_bytes())?;
    adb_spec(&old_config, &intent.device, state)?;
    if intent.configuration.as_bytes() == snapshot && intent.device == device {
        // A crash after intent replacement but before checkpoint publication is
        // safe to resume: every access approval and observation is requested anew.
        return Ok(intent.revision);
    }
    eprintln!("The TV configuration changed. Updating it requires stopping the old owned controller and approving TV and tunnel access again.");
    if terminal
        .line(
            "Stop the old controller and use the changed configuration? [y/N] ",
            false,
            ctx,
        )?
        .as_deref()
        != Some("y")
    {
        return Err("approval_rejected");
    }
    ctx.check().map_err(|e| e.code)?;
    let tunnel_state = state.join("tunnel");
    match tunnel_state.symlink_metadata() {
        Ok(_) => {
            let spec = TunnelSpec {
                binary: resolve_tunnel(state, ctx)?.ok_or("tunnel_client_missing")?,
                alias: format!("tv-{}", saved.installation_id),
                tunnel_id: saved
                    .tunnel_id
                    .clone()
                    .ok_or("tunnel_configuration_missing")?,
                runtime_key_env: "TV_CONTROL_TUNNEL_KEY".into(),
                state_dir: tunnel_state,
                mcp_binary: intent.controller_binary.clone(),
                configuration: saved.configuration_path.clone(),
                bridge_state_dir: Some(state.to_owned()),
                executable_path: std::env::var_os("PATH").ok_or("executable_path_missing")?,
            };
            let process = BoundedProcess {
                max_output: 65_536,
                poll_ms: 10,
            };
            let tunnel = OfficialTunnel { process: &process };
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
        // Intent is published before the tunnel is created. If creation never
        // began there is no owned runtime to stop; an unsafe/partial state fails.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("tunnel_state_unreadable"),
    }
    ctx.check().map_err(|e| e.code)?;
    if private_read(path)? != snapshot {
        return Err("configuration_changed_restart_setup");
    }
    ProofStore::open(&state.join("verification"))?.invalidate()?;
    let revision = random_token()?;
    let replacement = RuntimeIntent {
        schema: 1,
        installation_id: saved.installation_id.clone(),
        revision: revision.clone(),
        device: device.into(),
        configuration: std::str::from_utf8(snapshot)
            .map_err(|_| "configuration_invalid_utf8")?
            .into(),
        controller_binary: controller_binary.to_owned(),
    };
    let replacement =
        serde_json::to_vec(&replacement).map_err(|_| "runtime_intent_encode_failed")?;
    tv_adapters::runtime_intent::replace_after_stop(state, &bytes, &replacement)?;
    Ok(revision)
}
pub fn bridge(path: &Path, state: &Path) -> Result<i32, &'static str> {
    let snapshot = private_read(path)?;
    let intent: RuntimeIntent = serde_json::from_slice(&private_read_limit(
        &state.join("runtime-intent.json"),
        6_400_000,
    )?)
    .map_err(|_| "runtime_intent_invalid")?;
    if intent.schema != 1
        || intent.configuration.as_bytes() != snapshot
        || !tv_domain::identifier(&intent.installation_id)
        || !tv_domain::identifier(&intent.revision)
        || !tv_domain::identifier(&intent.device)
    {
        return Err("runtime_intent_mismatch");
    }
    let config = Arc::new(Config::from_json(&snapshot)?);
    let clock = SystemClock::default();
    let interrupts = Interrupts::install()?;
    let ctx = Context {
        clock: &clock,
        cancellation: &interrupts,
        deadline_monotonic_ms: 10_000,
    };
    verify_adb_dependency(&config, state, &ctx)?;
    let mut runtime = OwnedAdbRuntime::start(adb_spec(&config, &intent.device, state)?, true, &ctx)
        .map_err(|e| e.code)?;
    if runtime.connect(&ctx).map_err(|e| e.code)? != AdbAuthorization::Authorized {
        return Err("tv_authorization_pending");
    }
    let generation = tv_adapters::runtime_intent::start_generation(
        state,
        &intent.installation_id,
        &intent.revision,
        &intent.device,
    )?;
    let journal = path
        .parent()
        .ok_or("configuration_path_invalid")?
        .join(".tv-control-operations");
    let (legacy, generic) = crate::mcp_services(config.clone(), &journal)?;
    crate::mcp::serve_bridge(
        legacy,
        generic,
        config,
        state.to_owned(),
        snapshot,
        generation.generation,
        &interrupts,
    )?;
    Ok(0)
}

/// Explicit lifecycle command; stops only the exact owned official alias and bridge.
pub fn disconnect(
    path: &Path,
    as_json: bool,
    expected_launcher: Option<&Path>,
) -> Result<i32, &'static str> {
    let path = path
        .canonicalize()
        .map_err(|_| "configuration_unreadable")?;
    let state = path
        .parent()
        .ok_or("configuration_path_invalid")?
        .join("connection");
    let store = Store::open(&state)?;
    let checkpoint = store.load()?.ok_or("connection_progress_missing")?;
    let id = checkpoint.tunnel_id.ok_or("tunnel_configuration_missing")?;
    let intent: RuntimeIntent = serde_json::from_slice(&private_read_limit(
        &state.join("runtime-intent.json"),
        6_400_000,
    )?)
    .map_err(|_| "runtime_intent_invalid")?;
    // Multiple installation prefixes may share the default configuration. An
    // installer may stop only the controller approved for its exact launcher.
    // Check before resolving or invoking any tunnel helper.
    if expected_launcher.is_some_and(|launcher| launcher != intent.controller_binary) {
        if as_json {
            println!(
                "{}",
                json!({"status":"not_owned","controller_stopped":false})
            );
        } else {
            println!("Controller belongs to another installation; left running.");
        }
        return Ok(0);
    }
    let clock = SystemClock::default();
    let interrupts = Interrupts::install()?;
    let ctx = Context {
        clock: &clock,
        cancellation: &interrupts,
        deadline_monotonic_ms: 10_000,
    };
    let spec = TunnelSpec {
        binary: resolve_tunnel(&state, &ctx)?.ok_or("tunnel_client_missing")?,
        alias: format!("tv-{}", checkpoint.installation_id),
        tunnel_id: id,
        runtime_key_env: "TV_CONTROL_TUNNEL_KEY".into(),
        state_dir: state.join("tunnel"),
        mcp_binary: intent.controller_binary,
        configuration: path,
        bridge_state_dir: Some(state),
        executable_path: std::env::var_os("PATH").ok_or("executable_path_missing")?,
    };
    let process = BoundedProcess {
        max_output: 65536,
        poll_ms: 10,
    };
    let stopped = OfficialTunnel { process: &process }
        .stop(&spec, true, &ctx)
        .map_err(|e| e.code)?;
    if stopped.process_running {
        return Err("controller_stop_unverified");
    }
    if as_json {
        println!(
            "{}",
            json!({"status":"stopped","runtime_running":false,"credentials_removed":false})
        );
    } else {
        println!("Controller stopped. Private configuration and TV trust were preserved.");
    }
    Ok(0)
}
