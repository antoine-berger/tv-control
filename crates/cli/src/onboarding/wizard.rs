//! Guided configuration workflow, with explicit choices and checkpoint publication.
use super::dependencies::executable;
use super::discovery::{
    discover_on, discovery_error, ensure_fresh, manual_candidate, refresh_selected,
    select_interface, source,
};
use super::presentation::{
    candidate_json, choose, display_label, emit, guidance, prompt, readiness,
};
use super::{dependencies, progress, Options};
use serde_json::json;
use std::path::PathBuf;
use tv_adapters::{config::Config, runtime::SystemClock, setup};
use tv_application::{onboarding, Clock};
use tv_domain::onboarding::{ServiceKind, SetupReadiness};

pub(super) fn setup_command(input: &Options) -> Result<i32, &'static str> {
    let mut owned = input.clone();
    let output = PathBuf::from(input.get("--output").unwrap_or("tv-control.local.json"));
    if input.guided && !input.dry && output.symlink_metadata().is_ok() {
        use std::os::unix::fs::PermissionsExt;
        let meta = output
            .symlink_metadata()
            .map_err(|_| "existing_config_unreadable")?;
        if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
            return Err("existing_config_not_private");
        }
        let config = Config::from_json(&setup::read_bounded(&output)?)?;
        dependencies::verify_configuration(&config, &output)?;
        emit(
            json!({"status":"already_configured","message":"Your saved configuration is valid. TV authorization and ChatGPT connection still require explicit verification; no network access was performed.","output":output,"control_verified":false,"executed":false}),
            input.json,
        );
        return Ok(0);
    }
    if input.dry && input.get("--fixture").is_none() {
        emit(
            json!({"status":"dry_run","message":"Dry run: no discovery, configuration write or connection performed.","next_steps":["Choose an interface, discover displays, choose a device, then save its local configuration.","Use --fixture with --dry-run for an offline full preview."],"network_access":false,"writes":false}),
            input.json,
        );
        return Ok(0);
    }
    let clock = SystemClock::default();
    let port = source(input)?;
    let mut progress = progress::Progress::open(input)?;
    if let Some(progress) = progress.as_mut() {
        progress.restore(&mut owned, &*port)?;
    }
    let options = &owned;
    let interface = select_interface(options, &*port)?;
    if let Some(progress) = progress.as_mut() {
        progress.interface(&interface, &*port)?;
    }
    let mut manual = options.get("--manual-endpoint").is_some();
    let candidates = if manual {
        vec![]
    } else {
        if !options.json {
            eprintln!("Searching for TVs on {interface}... (Ctrl-C to cancel)");
        }
        match discover_on(options, &*port, &clock, &interface) {
            Ok(candidates) => candidates,
            Err(error) => {
                discovery_error(error, &interface, options.json);
                if options.json {
                    return Ok(1);
                }
                if !matches!(
                    error.code,
                    "mdns_send_failed"
                        | "ssdp_send_failed"
                        | "discovery_socket_failed"
                        | "mdns_socket_failed"
                        | "discovery_receive_failed"
                        | "expired"
                ) {
                    return Err(error.code);
                }
                if prompt(
                    "Enter this TV's connection details instead? Type manual, or Enter to stop:",
                )? != "manual"
                {
                    return Ok(2);
                }
                manual = true;
                vec![]
            }
        }
    };
    let saved_device = owned.get("--device").map(str::to_owned);
    let saved_name = owned.get("--name").map(str::to_owned);
    if input.get("--device").is_none()
        && owned.get("--device").is_some_and(|id| {
            !candidates.iter().any(|c| {
                c.id == id
                    && progress
                        .as_ref()
                        .is_none_or(|p| p.matches_target(c, clock.monotonic_ms()))
            })
        })
        && !manual
    {
        eprintln!(
            "The saved display is no longer advertised with the same identity at that address. Choose a current display."
        );
        owned.values.remove("--device");
        if input.get("--name").is_none() {
            owned.values.remove("--name");
        }
        if let Some(progress) = progress.as_mut() {
            progress.clear_target()?;
        }
    }
    let options = &owned;
    let candidate = if manual || candidates.is_empty() {
        if options.json && options.get("--manual-endpoint").is_none() {
            emit(
                json!({"status":"no_devices_found","message":"No displays advertised. No configuration saved.","next_steps":guidance(),"candidates":[],"interface":interface}),
                true,
            );
            return Ok(2);
        }
        manual = true;
        // Bind manual fallback to the interface chosen earlier in this invocation.
        let mut bound = Options {
            values: options.values.clone(),
            ..Options::default()
        };
        bound.values.insert("--interface".into(), interface.clone());
        manual_candidate(&bound, &*port, !options.json, None)?
    } else if let Some(id) = options.get("--device") {
        candidates
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or("device_not_found_rediscover")?
    } else if options.json {
        emit(
            json!({"status":"device_selection_required","message":"Choose a discovered device ID and repeat setup.","interface":interface,"candidates":candidates.iter().map(candidate_json).collect::<Vec<_>>()}),
            true,
        );
        return Ok(2);
    } else {
        let n = choose(
            &candidates,
            |c| format!("{} ({})", display_label(c), c.address),
            "Choose the display:",
        )?;
        candidates[n].clone()
    };
    let selected = candidate.clone();
    let mut plan = onboarding::plan_setup(candidate, 0);
    if plan.readiness == SetupReadiness::DiscoveryExpired
        || (!manual && ensure_fresh(&plan, &clock).is_err())
    {
        if let Some(progress) = progress.as_mut() {
            progress.target(&onboarding::plan_setup(selected.clone(), 0), false)?;
        }
        let Some(fresh) = refresh_selected(options, &*port, &clock, &interface, &selected)? else {
            return Ok(2);
        };
        plan = fresh;
    }
    if plan.endpoint.is_none()
        && !options.json
        && matches!(
            plan.readiness,
            SetupReadiness::DebuggingRequired | SetupReadiness::AddressUnresolved
        )
    {
        eprintln!("This display has no verified ADB connection endpoint. Pairing ports are never control ports.");
        if prompt("Enter a connection port from this TV's debugging settings? Type manual, or press Enter to stop:")?=="manual" {
            let mut bound=Options{values:options.values.clone(),..Options::default()};
            bound.values.insert("--interface".into(),interface.clone());
            let candidate=manual_candidate(&bound,&*port,true,Some(plan.candidate.address))?;
            let manual_port=candidate.services[0].port;
            if plan.candidate.services.iter().any(|s|s.kind==ServiceKind::AdbTlsPairing && s.port==manual_port) {return Err("pairing_port_cannot_be_control_endpoint");}
            plan=onboarding::plan_setup(candidate,clock.monotonic_ms());
            manual=true;
        }
    }
    if plan.endpoint.is_none() {
        emit(
            json!({"status":readiness(&plan.readiness),"message":"Display detected; device setup is required before a control configuration can be saved.","candidate":candidate_json(&plan.candidate),"manual_fallback":{"address":plan.candidate.address.to_string(),"interface":interface,"requires":["connection_port_from_tv_settings_not_pairing_port","explicit_legacy_or_tls_protocol"]},"next_steps":guidance(),"writes":false}),
            options.json,
        );
        return Ok(2);
    }
    if let Some(progress) = progress.as_mut() {
        progress.target(&plan, manual)?;
    }
    let adb = dependencies::adb(options)?;
    let curl = executable(options, "--curl", "curl")?;
    let server_port: u16 = options
        .get("--server-port")
        .unwrap_or("15037")
        .parse()
        .map_err(|_| "invalid_server_port")?;
    let mut config = setup::configuration(&plan, adb, curl, server_port)?;
    let name = if let Some(name) = options.get("--name").or_else(|| {
        if saved_device.as_deref() == Some(plan.candidate.id.as_str()) {
            saved_name.as_deref()
        } else {
            None
        }
    }) {
        Some(name.to_string())
    } else if options.guided && !options.json {
        let default = display_label(&plan.candidate);
        let value = prompt(&format!("Local name [{default}] (Enter to keep):"))?;
        Some(if value.is_empty() {
            default.to_string()
        } else {
            value
        })
    } else {
        None
    };
    if let Some(tv_adapters::config::Device::Adb { display_name, .. }) =
        config.devices.values_mut().next()
    {
        *display_name = name.clone();
    }
    config.validate()?;
    if let Some(progress) = progress.as_mut() {
        progress.dependencies(&config)?;
    }
    let output = PathBuf::from(options.get("--output").unwrap_or("tv-control.local.json"));
    let preview = json!({"status":"confirmation_required","message":"Configuration preview ready. Device control remains unverified.","candidate":candidate_json(&plan.candidate),"configuration":config,"output":output,"simulation":options.get("--fixture").is_some(),"next_steps":guidance(),"writes":false});
    if options.dry || (options.json && !options.confirm) {
        emit(preview, options.json);
        return Ok(0);
    }
    if !manual && ensure_fresh(&plan, &clock).is_err() {
        let Some(fresh) = refresh_selected(options, &*port, &clock, &interface, &selected)? else {
            return Ok(2);
        };
        plan = fresh;
        // Rebuild from the revalidated endpoint, keeping the local name and dependencies.
        config = setup::configuration(
            &plan,
            config.executables.adb.clone(),
            config.executables.curl.clone(),
            server_port,
        )?;
        if let Some(tv_adapters::config::Device::Adb { display_name, .. }) =
            config.devices.values_mut().next()
        {
            *display_name = name;
        }
        if let Some(progress) = progress.as_mut() {
            progress.target(&plan, false)?;
            progress.dependencies(&config)?;
        }
    }
    if !options.json && !options.guided {
        eprintln!(
            "ADB executable: {:?}; dedicated loopback server port: {}",
            config.executables.adb, server_port
        );
        eprintln!("Will save {} for {}. No media app or volume calibration is assumed. No device access is granted.",output.display(),plan.endpoint.as_deref().unwrap_or("unknown"));
    }
    let mut bytes =
        serde_json::to_vec_pretty(&config).map_err(|_| "configuration_encode_failed")?;
    bytes.push(b'\n');
    ensure_fresh(&plan, &clock)?;
    let status = setup::publish(&output, &bytes, options.resume)?;
    if let Some(progress) = progress.as_mut() {
        progress.completed()?;
    }
    if options.guided {
        emit(
            json!({"status":status,"message":"TV selected.","output":output,"control_verified":false,"executed":false}),
            options.json,
        );
        return Ok(0);
    }
    let binary = std::env::current_exe().map_err(|_| "executable_unavailable")?;
    let commands = json!([
        [
            config.executables.adb.to_string_lossy(),
            "-L",
            format!("tcp:localhost:{server_port}"),
            "--one-device",
            "TVCONTROL_NO_USB",
            "server",
            "nodaemon"
        ],
        [
            config.executables.adb.to_string_lossy(),
            "-H",
            "127.0.0.1",
            "-P",
            server_port.to_string(),
            "connect",
            plan.endpoint.as_deref().unwrap_or("")
        ],
        [
            binary.to_string_lossy(),
            "doctor",
            output.to_string_lossy(),
            "--probe"
        ]
    ]);

    emit(
        json!({"status":status,"message":"Configuration saved. ADB authorization and control are still unverified.","output":output,"simulation":options.get("--fixture").is_some(),"device":candidate_json(&plan.candidate),"control_verified":false,"executed":false,"commands":commands,"next_steps":["Review these commands before running them; the foreground daemon, connection and read-only probe require explicit authorization. Run connect/probe in a second terminal.","Prepare an exclusively owned compatible ADB daemon and authorize the selected TV explicitly; see docs/ONBOARDING.md.","Rediscover after a TV reboot or wireless-debugging change; dynamic ports can change.","Run doctor CONFIG for offline checks, or doctor CONFIG --probe only when read-only device access is authorized.","Run connect-chatgpt CONFIG to prepare the manual stdio tunnel and plugin steps."]}),
        options.json,
    );
    Ok(0)
}
