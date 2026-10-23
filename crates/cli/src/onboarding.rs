//! Human and agent incoming adapters. Setup may acquire approved dependencies,
//! but never executes ADB, authenticates or starts a tunnel.
mod dependencies;
mod progress;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Read, Write},
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
    sync::Arc,
};
use tv_adapters::{
    config::Config,
    discovery::{fixture::FixtureDiscovery, LocalDiscovery},
    runtime::{CancelFlag, SystemClock},
    setup,
};
use tv_application::{
    onboarding::{self, NetworkDiscovery},
    Clock, Context,
};
use tv_domain::{onboarding::*, Action, Request, Status};
#[derive(Clone, Default)]
struct Options {
    values: BTreeMap<String, String>,
    json: bool,
    dry: bool,
    confirm: bool,
    resume: bool,
    probe: bool,
    no_checkpoint: bool,
    guided: bool,
}
const MAX_DISCOVERY_TIMEOUT_MS: u64 = 10_000;
struct DiscoveryTiming {
    window_ms: u64,
    timeout_ms: u64,
}
impl Default for DiscoveryTiming {
    fn default() -> Self {
        Self {
            window_ms: 2_000,
            timeout_ms: MAX_DISCOVERY_TIMEOUT_MS,
        }
    }
}
impl DiscoveryTiming {
    fn from_options(options: &Options) -> tv_domain::PortResult<Self> {
        let defaults = Self::default();
        let window_ms = options
            .get("--duration-ms")
            .map(str::parse::<u64>)
            .transpose()
            .map_err(|_| tv_domain::Failure::before("invalid_discovery_duration"))?
            .unwrap_or(defaults.window_ms);
        let timeout_ms = options
            .get("--timeout-ms")
            .map(str::parse::<u64>)
            .transpose()
            .map_err(|_| tv_domain::Failure::before("invalid_discovery_timeout"))?
            .unwrap_or(defaults.timeout_ms);
        if !(100..=5000).contains(&window_ms) {
            return Err(tv_domain::Failure::before(
                "discovery_duration_out_of_range",
            ));
        }
        if timeout_ms <= window_ms || timeout_ms > MAX_DISCOVERY_TIMEOUT_MS {
            return Err(tv_domain::Failure::before("discovery_timeout_out_of_range"));
        }
        Ok(Self {
            window_ms,
            timeout_ms,
        })
    }
}
impl Options {
    fn parse(args: &[String]) -> Result<Self, &'static str> {
        let mut out = Self::default();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--json" => out.json = true,
                "--dry-run" => out.dry = true,
                "--confirm" => out.confirm = true,
                "--resume" => out.resume = true,
                "--probe" => out.probe = true,
                "--no-checkpoint" => out.no_checkpoint = true,
                "--guided" => out.guided = true,
                key @ ("--fixture" | "--interface" | "--duration-ms" | "--timeout-ms"
                | "--device" | "--output" | "--adb" | "--curl" | "--server-port"
                | "--manual-endpoint" | "--manual-kind" | "--tunnel-id" | "--checkpoint"
                | "--name") => {
                    i += 1;
                    let value = args.get(i).ok_or("option_value_missing")?;
                    if value.starts_with("--")
                        || out.values.insert(key.into(), value.clone()).is_some()
                    {
                        return Err("invalid_or_duplicate_option");
                    }
                }
                _ => return Err("unknown_onboarding_option"),
            }
            i += 1;
        }
        Ok(out)
    }
    fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
}
fn fail(error: tv_domain::Failure) -> &'static str {
    error.code
}
fn emit(value: Value, as_json: bool) {
    if as_json {
        println!("{value}");
    } else {
        println!(
            "{}",
            value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Done.")
        );
        if let Some(steps) = value.get("next_steps").and_then(Value::as_array) {
            for step in steps {
                if let Some(text) = step.as_str() {
                    println!("  {text}");
                }
            }
        }
        if let Some(commands) = value.get("commands").and_then(Value::as_array) {
            for command in commands {
                if let Some(args) = command.as_array() {
                    let line = args
                        .iter()
                        .filter_map(Value::as_str)
                        .map(setup::shell_quote)
                        .collect::<Result<Vec<_>, _>>();
                    if let Ok(line) = line {
                        println!("  {}", line.join(" "));
                    }
                }
            }
        }
    }
}
fn prompt(text: &str) -> Result<String, &'static str> {
    eprint!("{text} ");
    io::stderr().flush().map_err(|_| "terminal_unavailable")?;
    let mut line = String::new();
    let n = io::stdin()
        .lock()
        .take(1025)
        .read_line(&mut line)
        .map_err(|_| "input_unreadable")?;
    if n == 0 {
        return Err("setup_cancelled");
    }
    if n >= 1024 {
        return Err("input_too_long");
    }
    let text = line.trim();
    if text.eq_ignore_ascii_case("q") {
        return Err("setup_cancelled");
    }
    Ok(text.to_string())
}
fn choose<T>(
    items: &[T],
    label: impl Fn(&T) -> String,
    title: &str,
) -> Result<usize, &'static str> {
    if items.is_empty() {
        return Err("no_choices_available");
    }
    eprintln!("{title} (q cancels)");
    for (i, item) in items.iter().enumerate() {
        eprintln!("  {}) {}", i + 1, label(item));
    }
    let selected: usize = prompt("Choose a number:")?
        .parse()
        .map_err(|_| "invalid_selection")?;
    if selected == 0 || selected > items.len() {
        return Err("invalid_selection");
    }
    Ok(selected - 1)
}
fn source(options: &Options) -> Result<Box<dyn NetworkDiscovery>, &'static str> {
    match options.get("--fixture") {
        Some(path) => Ok(Box::new(
            FixtureDiscovery::from_json(&setup::read_bounded(Path::new(path))?).map_err(fail)?,
        )),
        None => Ok(Box::new(LocalDiscovery)),
    }
}
fn interface_json(i: &LocalInterface) -> Value {
    json!({"selector":format!("{}@{}",i.name,i.address),"name":i.name,"address":i.address.to_string(),"netmask":i.netmask.to_string()})
}
fn service_name(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::AdbLegacy => "adb_legacy_unencrypted",
        ServiceKind::AdbTlsConnect => "adb_tls_connect",
        ServiceKind::AdbTlsPairing => "adb_tls_pairing_only",
        ServiceKind::Dial => "dial_advertisement_only",
    }
}
fn display_label(c: &Candidate) -> &str {
    if c.services.iter().any(|s| {
        matches!(
            s.kind,
            ServiceKind::AdbLegacy | ServiceKind::AdbTlsConnect | ServiceKind::AdbTlsPairing
        )
    }) {
        if c.label.to_ascii_lowercase().starts_with("adb-") || c.label.is_empty() {
            "Android device (ADB)"
        } else {
            &c.label
        }
    } else {
        "Unidentified media device"
    }
}
fn candidate_json(c: &Candidate) -> Value {
    json!({"id":c.id,"label":display_label(c),"diagnostic_label":c.label,"address":c.address.to_string(),"evidence":"advertised_not_verified","authorized":null,"control_verified":false,"services":c.services.iter().map(|s|json!({"kind":service_name(s.kind),"port":s.port,"instance":s.instance})).collect::<Vec<_>>()})
}
#[cfg(test)]
fn discover(
    options: &Options,
    port: &dyn NetworkDiscovery,
    clock: &dyn Clock,
) -> Result<(String, Vec<Candidate>), &'static str> {
    let selected = select_interface(options, port)?;
    let candidates = discover_on(options, port, clock, &selected).map_err(fail)?;
    Ok((selected, candidates))
}
fn select_interface(
    options: &Options,
    port: &dyn NetworkDiscovery,
) -> Result<String, &'static str> {
    let interfaces = port.interfaces().map_err(fail)?;
    let selected = if let Some(name) = options.get("--interface") {
        name.to_string()
    } else if options.json {
        return Err("interface_required");
    } else if options.guided && interfaces.len() == 1 {
        format!("{}@{}", interfaces[0].name, interfaces[0].address)
    } else {
        let n = choose(
            &interfaces,
            |i| format!("{} ({})", i.name, i.address),
            "Choose the LAN interface shared with the TV; avoid WAN/VPN:",
        )?;
        format!("{}@{}", interfaces[n].name, interfaces[n].address)
    };
    let matches: Vec<_> = interfaces
        .iter()
        .filter(|i| i.name == selected || format!("{}@{}", i.name, i.address) == selected)
        .collect();
    if matches.len() != 1 {
        return Err("interface_missing_or_ambiguous");
    }
    Ok(format!("{}@{}", matches[0].name, matches[0].address))
}
fn discover_on(
    options: &Options,
    port: &dyn NetworkDiscovery,
    clock: &dyn Clock,
    selected: &str,
) -> tv_domain::PortResult<Vec<Candidate>> {
    let timing = DiscoveryTiming::from_options(options)?;
    // Start both bounds only after interface selection and all human input.
    let start = clock.monotonic_ms();
    let window_end = start.saturating_add(timing.window_ms);
    let flag = CancelFlag::default();
    let ctx = Context {
        clock,
        cancellation: &flag,
        deadline_monotonic_ms: start.saturating_add(timing.timeout_ms),
    };
    onboarding::discover(port, selected, window_end, &ctx)
}
fn discovery_error(error: tv_domain::Failure, interface: &str, as_json: bool) {
    if error.code == "expired" || error.code == "cancelled" {
        let message = if error.code == "expired" {
            "TV search exceeded its time limit. Retry setup or use the manual fallback."
        } else {
            "TV search cancelled."
        };
        if as_json {
            emit(
                json!({"status":if error.code=="expired"{"timeout"}else{"cancelled"},"code":error.code,"interface":interface,"message":message}),
                true,
            );
        } else {
            eprintln!("{message}");
        }
        return;
    }
    if as_json {
        emit(
            json!({"status":"error","code":error.code,"os_error":error.os_error,"interface":interface,"cause":"not_established","next_steps":["Discovery failed. Resume setup to retry explicitly, or use its guided manual endpoint fallback.","No TV authorization or connection was performed."]}),
            true,
        );
    } else {
        eprintln!(
            "Discovery failed: {} (OS error: {}). The cause is not established.",
            error.code,
            error
                .os_error
                .map(|n| n.to_string())
                .unwrap_or_else(|| "unavailable".into())
        );
    }
}
fn executable(options: &Options, key: &str, name: &str) -> Result<PathBuf, &'static str> {
    use std::os::unix::fs::PermissionsExt;
    let supplied = options.get(key).map(PathBuf::from);
    let candidates = if let Some(path) = supplied {
        vec![path]
    } else {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .filter(|p| p.is_absolute())
            .map(|p| p.join(name))
            .collect()
    };
    for path in candidates {
        if let Ok(meta) = path.metadata() {
            if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                return path.canonicalize().map_err(|_| "executable_unreadable");
            }
        }
    }
    Err(if name == "adb" {
        "adb_missing_install_official_platform_tools"
    } else {
        "curl_missing_install_system_package"
    })
}
fn readiness(value: &SetupReadiness) -> &'static str {
    match value {
        SetupReadiness::AuthorizationUnverified => "authorization_unverified",
        SetupReadiness::PairingRequired => "pairing_required",
        SetupReadiness::DebuggingRequired => "debugging_required",
        SetupReadiness::AmbiguousEndpoint => "ambiguous_endpoint",
        SetupReadiness::AddressUnresolved => "address_unresolved",
        SetupReadiness::DiscoveryExpired => "discovery_expired_rediscover",
    }
}
fn guidance() -> Vec<&'static str> {
    vec![
 "Detection is an untrusted advertisement, not proof of model, Android version, control support or authorization.",
 "Enable debugging only after reviewing access on the TV. Wireless debugging availability depends on Android TV version/vendor; do not assume Android 12 TV supports TLS pairing.",
 "Fire OS: enable ADB debugging in Developer Options and approve the host on the TV. Vega/non-Android Fire TV models are not supported by this ADB adapter.",
 "A pairing port is temporary and must never be used for control. Pair explicitly using maintained platform-tools, then rediscover the separate TLS connection port.",
 "If nothing appears: check the same LAN, chosen interface, multicast isolation and TV wake state; use the guided manual endpoint fallback only when needed.",
 "Legacy TCP ADB is unencrypted. This setup does not start a daemon, connect, pair or change TV settings."]
}
fn manual_candidate(
    options: &Options,
    port: &dyn NetworkDiscovery,
    interactive: bool,
    selected_address: Option<Ipv4Addr>,
) -> Result<Candidate, &'static str> {
    let endpoint = if let Some(value) = options.get("--manual-endpoint") {
        value.to_string()
    } else if interactive {
        eprintln!("{}", guidance().join("\n"));
        if let Some(address) = selected_address {
            let port:u16=prompt("Enter this display's connection port from its debugging settings (not a pairing port), or q:")?.parse().map_err(|_|"invalid_manual_endpoint")?;
            format!("{address}:{port}")
        } else {
            prompt("Enter the TV's connection IPv4:port from its debugging settings (not the pairing port), or q:")?
        }
    } else {
        return Err("no_devices_found");
    };
    let endpoint: SocketAddrV4 = endpoint.parse().map_err(|_| "invalid_manual_endpoint")?;
    let address = *endpoint.ip();
    if endpoint.port() == 0
        || !port
            .interfaces()
            .map_err(fail)?
            .iter()
            .filter(|i| {
                options
                    .get("--interface")
                    .is_none_or(|n| i.name == n || format!("{}@{}", i.name, i.address) == n)
            })
            .any(|i| i.contains(address))
    {
        return Err("manual_endpoint_outside_selected_lan");
    }
    let kind = if let Some(kind) = options.get("--manual-kind") {
        kind.to_string()
    } else if interactive {
        prompt("Connection protocol: type legacy (unencrypted) or tls:")?
    } else {
        return Err("manual_protocol_required");
    };
    let kind = match kind.as_str() {
        "legacy" => ServiceKind::AdbLegacy,
        "tls" => ServiceKind::AdbTlsConnect,
        _ => return Err("invalid_manual_protocol"),
    };
    Ok(Candidate {
        id: format!("screen_{}", address.to_string().replace('.', "_")),
        label: "Manually specified display".into(),
        address,
        services: vec![AdvertisedService {
            kind,
            port: Some(endpoint.port()),
            instance: None,
            valid_until_monotonic_ms: None,
        }],
    })
}
fn ensure_fresh(plan: &SetupPlan, clock: &dyn Clock) -> Result<(), &'static str> {
    if plan.candidate.services.iter().any(|s| {
        s.valid_until_monotonic_ms
            .is_some_and(|until| until <= clock.monotonic_ms())
    }) {
        return Err("discovery_expired_rediscover");
    }
    Ok(())
}
fn refresh_selected(
    options: &Options,
    port: &dyn NetworkDiscovery,
    clock: &dyn Clock,
    interface: &str,
    selected: &Candidate,
) -> Result<Option<SetupPlan>, &'static str> {
    loop {
        if !options.json {
            eprintln!("The selected display advertisement expired. Refreshing its endpoint on {interface}...");
        }
        let result = discover_on(options, port, clock, interface).and_then(|candidates| {
            onboarding::refresh_setup(selected, candidates, clock.monotonic_ms())
        });
        match result {
            Ok(plan) => return Ok(Some(plan)),
            Err(error) => {
                if options.json {
                    emit(
                        json!({"status":"refresh_required","code":error.code,"os_error":error.os_error,"message":"The selected display could not be revalidated. Retry setup with the same selection; no configuration was saved.","configuration_saved":false,"control_verified":false,"executed":false}),
                        true,
                    );
                    return Ok(None);
                }
                eprintln!("Could not refresh the selected display: {} (OS error: {}). No configuration saved; local setup progress is retained when checkpoints are enabled.", error.code, error.os_error.map(|n| n.to_string()).unwrap_or_else(|| "unavailable".into()));
                if prompt("Retry this display on the same LAN? Enter to retry, or q to stop:")?
                    .is_empty()
                {
                    continue;
                }
                return Err("invalid_selection");
            }
        }
    }
}
fn setup_command(input: &Options) -> Result<i32, &'static str> {
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
pub fn connection_plan(
    config: &Path,
    binary: &Path,
    tunnel_id: Option<&str>,
) -> Result<Value, &'static str> {
    let id = tunnel_id.unwrap_or("TUNNEL_ID_FROM_PLATFORM");
    if tunnel_id.is_some()
        && (!id.starts_with("tunnel_")
            || id.len() > 128
            || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
    {
        return Err("invalid_tunnel_id");
    }
    let binary = binary.to_str().ok_or("non_utf8_executable")?;
    let config = config.to_str().ok_or("non_utf8_configuration")?;
    let command = format!(
        "{} mcp {}",
        setup::shell_quote(binary)?,
        setup::shell_quote(config)?
    );
    Ok(
        json!({"status":"manual_authorization_required","message":"Connection plan prepared; no tunnel, credential or plugin created.","executed":false,"commands":[["tunnel-client","help","quickstart"],["tunnel-client","init","--sample","sample_mcp_stdio_local","--profile","tv-control","--tunnel-id",id,"--mcp-command",command],["tunnel-client","doctor","--profile","tv-control","--explain"],["tunnel-client","run","--profile","tv-control"]],"next_steps":["Download the official openai/tunnel-client release and verify its published checksum; it is a separate dependency.","In Platform, select the intended organization and ChatGPT workspace. Reuse or create a tunnel explicitly. Creation needs Tunnels Read + Manage; runtime needs Read + Use.","Keep the restricted runtime key in CONTROL_PLANE_API_KEY or the supported credential store. Never paste it into this CLI, a command argument or logs.","Replace TUNNEL_ID_FROM_PLATFORM with the authorized tunnel ID before running init. Review current tunnel-client help; run it only after approving persistent access.","In ChatGPT Plugins: Add custom MCP server → Tunnel → select the authorized tunnel; review authentication and the risk warning, then create and install the plugin.","Select the installed plugin in the intended dot/chat. Verify tv_capabilities is callable there before any TV command. Availability depends on workspace permissions.","Public source code does not install a public-directory plugin. Tunnel connection is private, and no latency guarantee is implied."],"documentation":"https://developers.openai.com/api/docs/guides/secure-mcp-tunnels"}),
    )
}
pub fn run(command: &str, args: &[String]) -> Result<i32, &'static str> {
    let (config_path, options_args) = if matches!(command, "connect-chatgpt" | "doctor") {
        let path = args.first().ok_or("configuration_required")?;
        if path.starts_with("--") {
            return Err("configuration_required");
        }
        (Some(PathBuf::from(path)), &args[1..])
    } else {
        (None, args)
    };
    let options = Options::parse(options_args)?;
    let allowed: &[&str] = match command {
        "doctor" => &["--device"],
        "connect-chatgpt" => &["--tunnel-id"],
        "interfaces" => &["--fixture"],
        "discover" => &["--fixture", "--interface", "--duration-ms", "--timeout-ms"],
        _ => &[
            "--fixture",
            "--interface",
            "--duration-ms",
            "--timeout-ms",
            "--device",
            "--output",
            "--adb",
            "--curl",
            "--server-port",
            "--manual-endpoint",
            "--manual-kind",
            "--checkpoint",
            "--name",
        ],
    };
    if options
        .values
        .keys()
        .any(|key| !allowed.contains(&key.as_str()))
        || (options.probe && command != "doctor")
        || ((options.confirm || options.resume || options.no_checkpoint || options.guided)
            && command != "setup")
    {
        return Err("option_not_supported_for_command");
    }

    match command {
        "interfaces" => {
            let port = source(&options)?;
            let all = port.interfaces().map_err(fail)?;
            if options.json {
                emit(
                    json!({"status":"interfaces","interfaces":all.iter().map(interface_json).collect::<Vec<_>>(),"simulation":options.get("--fixture").is_some()}),
                    true,
                );
            } else {
                for i in all {
                    println!("{}@{}", i.name, i.address);
                }
            }
            Ok(0)
        }
        "discover" => {
            if options.dry && options.get("--fixture").is_none() {
                emit(
                    json!({"status":"dry_run","message":"Would send one mDNS query and one DIAL SSDP query on the explicitly selected IPv4 LAN interface.","network_access":false,"interface":options.get("--interface")}),
                    options.json,
                );
                return Ok(0);
            }
            let port = source(&options)?;
            let clock = SystemClock::default();
            let interface = select_interface(&options, &*port)?;
            let candidates = match discover_on(&options, &*port, &clock, &interface) {
                Ok(candidates) => candidates,
                Err(error) => {
                    discovery_error(error, &interface, options.json);
                    return Ok(1);
                }
            };
            if options.json {
                emit(
                    json!({"status":"discovered","interface":interface,"simulation":options.get("--fixture").is_some(),"candidates":candidates.iter().map(candidate_json).collect::<Vec<_>>(),"next_steps":if candidates.is_empty(){guidance()}else{vec!["Choose a candidate with setup; discovery does not verify control or grant access."]}}),
                    true,
                );
            } else {
                println!("{} display candidate(s), all unverified:", candidates.len());
                for c in candidates {
                    println!("  {}  {} ({})", c.id, display_label(&c), c.address);
                }
            }
            Ok(0)
        }
        "setup" => setup_command(&options),
        "connect-chatgpt" => {
            let path = config_path
                .ok_or("configuration_required")?
                .canonicalize()
                .map_err(|_| "configuration_unreadable")?;
            Config::from_json(&setup::read_bounded(&path)?)?;
            let binary = std::env::current_exe().map_err(|_| "executable_unavailable")?;
            emit(
                connection_plan(&path, &binary, options.get("--tunnel-id"))?,
                options.json,
            );
            Ok(0)
        }
        "doctor" => {
            let path = config_path.ok_or("configuration_required")?;
            let config = Arc::new(Config::from_json(&setup::read_bounded(&path)?)?);
            if options.probe && !options.dry {
                let alias = options.get("--device").unwrap_or("screen");
                let clock = SystemClock::default();
                let receipt = super::service(config).execute(
                    Request {
                        id: "onboarding_probe".into(),
                        device: alias.into(),
                        action: Action::Observe,
                        expires_at_unix_ms: clock.unix_ms() + 5000,
                    },
                    &CancelFlag::default(),
                );
                let success = receipt.status == Status::Observed;
                emit(
                    json!({"status":if success{"state_observed"}else{"probe_failed"},"message":"Read-only controller probe completed; no mutation or setup was requested.","receipt":tv_adapters::wire::receipt(&receipt),"next_steps":["A successful state read does not qualify playback, volume or hosted chat latency.","If the target is unavailable, check authorization, isolated daemon ownership and rediscover dynamic ports; no reconnect or pairing is attempted."]}),
                    options.json,
                );
                Ok(if success { 0 } else { 2 })
            } else {
                emit(
                    json!({"status":"configuration_valid","message":"Configuration is valid. Offline checks do not verify device access.","device_access":false,"control_verified":false,"executables":{"adb_present":config.executables.adb.is_file(),"curl_present":config.executables.curl.is_file()},"next_steps":["Authorize and prepare the dedicated ADB connection explicitly before doctor --probe.","No daemon, pairing, account access or tunnel was started."]}),
                    options.json,
                );
                Ok(0)
            }
        }
        _ => Err("unknown_onboarding_command"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    struct MenuClock(AtomicU64);
    impl Clock for MenuClock {
        fn unix_ms(&self) -> u64 {
            0
        }
        fn monotonic_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
        fn sleep_ms(&self, ms: u64) {
            self.0.fetch_add(ms, Ordering::SeqCst);
        }
    }
    #[test]
    fn slow_interface_choice_does_not_age_a_later_fresh_advertisement() {
        // Simulate 31 seconds spent at the interface menu before any query/reply.
        let clock = MenuClock(AtomicU64::new(31_000));
        let port = FixtureDiscovery::from_json(include_bytes!("../../../examples/discovery.json"))
            .unwrap();
        let mut options = Options::default();
        options
            .values
            .insert("--interface".into(), "lan-fixture".into());
        let (_, candidates) = discover(&options, &port, &clock).unwrap();
        let candidate = candidates
            .into_iter()
            .find(|c| c.id == "screen_192_0_2_10")
            .unwrap();
        let plan = onboarding::plan_setup(candidate, clock.monotonic_ms());
        assert!(plan.endpoint.is_some());
        clock.sleep_ms(1000); // The user now confirms a still-fresh snapshot.
        assert_eq!(ensure_fresh(&plan, &clock), Ok(()));
        clock.sleep_ms(29_000);
        assert_eq!(
            ensure_fresh(&plan, &clock),
            Err("discovery_expired_rediscover")
        );
    }
    #[test]
    fn search_window_and_hard_deadline_start_after_long_human_delay() {
        struct Probe;
        impl NetworkDiscovery for Probe {
            fn interfaces(&self) -> tv_domain::PortResult<Vec<LocalInterface>> {
                Ok(vec![LocalInterface {
                    name: "lan".into(),
                    address: Ipv4Addr::new(192, 0, 2, 1),
                    netmask: Ipv4Addr::new(255, 255, 255, 0),
                }])
            }
            fn discover(
                &self,
                _: &LocalInterface,
                end: u64,
                ctx: &Context<'_>,
            ) -> tv_domain::PortResult<Vec<Candidate>> {
                assert_eq!(ctx.clock.monotonic_ms(), 31_000);
                assert_eq!(end, 33_000);
                assert_eq!(ctx.deadline_monotonic_ms, 41_000);
                ctx.check()?;
                Ok(vec![])
            }
        }
        let clock = MenuClock(AtomicU64::new(0));
        clock.sleep_ms(31_000);
        assert!(discover_on(&Options::default(), &Probe, &clock, "lan")
            .unwrap()
            .is_empty());
    }
    #[test]
    fn discovery_timeout_configuration_is_bounded_and_independent() {
        let defaults = DiscoveryTiming::from_options(&Options::default()).unwrap();
        assert_eq!(defaults.window_ms, 2_000);
        assert_eq!(defaults.timeout_ms, 10_000);
        for timeout in [2_001, 5_000, 10_000] {
            let options = Options::parse(&["--timeout-ms".into(), timeout.to_string()]).unwrap();
            assert_eq!(
                DiscoveryTiming::from_options(&options).unwrap().timeout_ms,
                timeout
            );
        }
        for timeout in ["0", "1999", "2000", "10001", "18446744073709551615"] {
            let options = Options::parse(&["--timeout-ms".into(), timeout.into()]).unwrap();
            assert_eq!(
                DiscoveryTiming::from_options(&options).err().unwrap().code,
                "discovery_timeout_out_of_range"
            );
        }
        let options = Options::parse(&["--timeout-ms".into(), "invalid".into()]).unwrap();
        assert_eq!(
            DiscoveryTiming::from_options(&options).err().unwrap().code,
            "invalid_discovery_timeout"
        );
    }
    #[test]
    fn search_results_must_complete_before_the_ten_second_deadline() {
        struct Probe(u64);
        impl NetworkDiscovery for Probe {
            fn interfaces(&self) -> tv_domain::PortResult<Vec<LocalInterface>> {
                Ok(vec![LocalInterface {
                    name: "lan".into(),
                    address: Ipv4Addr::new(192, 0, 2, 1),
                    netmask: Ipv4Addr::new(255, 255, 255, 0),
                }])
            }
            fn discover(
                &self,
                _: &LocalInterface,
                end: u64,
                ctx: &Context<'_>,
            ) -> tv_domain::PortResult<Vec<Candidate>> {
                assert_eq!(end, 2_000);
                assert_eq!(ctx.deadline_monotonic_ms, 10_000);
                ctx.clock.sleep_ms(self.0);
                Ok(vec![])
            }
        }
        for elapsed in [3_000, 9_999, 10_000] {
            let clock = MenuClock(AtomicU64::new(0));
            let result = discover_on(&Options::default(), &Probe(elapsed), &clock, "lan");
            if elapsed < 10_000 {
                assert!(result.is_ok());
            } else {
                assert_eq!(result.unwrap_err().code, "expired");
            }
        }
    }
    #[test]
    fn opaque_adb_and_ssdp_server_values_are_not_presented_as_friendly_names() {
        let mut candidate = Candidate {
            id: "screen".into(),
            label: "adb-deadbeef".into(),
            address: Ipv4Addr::new(192, 0, 2, 10),
            services: vec![AdvertisedService {
                kind: ServiceKind::AdbLegacy,
                port: Some(5555),
                instance: None,
                valid_until_monotonic_ms: None,
            }],
        };
        assert_eq!(display_label(&candidate), "Android device (ADB)");
        candidate.label = "Living room".into();
        assert_eq!(display_label(&candidate), "Living room");
        candidate.services[0].kind = ServiceKind::Dial;
        candidate.label = "Linux2.6 UPnP1.1 server1.1".into();
        assert_eq!(display_label(&candidate), "Unidentified media device");
    }
}
