//! Human and agent incoming adapters. Setup never spawns ADB or an auth/tunnel client.
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Read, Write},
    net::SocketAddrV4,
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
#[derive(Default)]
struct Options {
    values: BTreeMap<String, String>,
    json: bool,
    dry: bool,
    confirm: bool,
    resume: bool,
    probe: bool,
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
                key @ ("--fixture" | "--interface" | "--duration-ms" | "--device" | "--output"
                | "--adb" | "--curl" | "--server-port" | "--manual-endpoint"
                | "--manual-kind" | "--tunnel-id") => {
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
fn candidate_json(c: &Candidate) -> Value {
    json!({"id":c.id,"label":c.label,"address":c.address.to_string(),"evidence":"advertised_not_verified","authorized":null,"control_verified":false,"services":c.services.iter().map(|s|json!({"kind":service_name(s.kind),"port":s.port,"instance":s.instance})).collect::<Vec<_>>()})
}
fn discover(
    options: &Options,
    port: &dyn NetworkDiscovery,
) -> Result<(String, Vec<Candidate>), &'static str> {
    let interfaces = port.interfaces().map_err(fail)?;
    let selected = if let Some(name) = options.get("--interface") {
        name.to_string()
    } else if options.json {
        return Err("interface_required");
    } else {
        let n = choose(
            &interfaces,
            |i| format!("{} ({})", i.name, i.address),
            "Choose the LAN interface shared with the TV; avoid WAN/VPN:",
        )?;
        format!("{}@{}", interfaces[n].name, interfaces[n].address)
    };
    let duration: u64 = options
        .get("--duration-ms")
        .unwrap_or("2000")
        .parse()
        .map_err(|_| "invalid_discovery_duration")?;
    if !(100..=5000).contains(&duration) {
        return Err("discovery_duration_out_of_range");
    }
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    let ctx = Context {
        clock: &clock,
        cancellation: &flag,
        deadline_monotonic_ms: clock.monotonic_ms() + duration,
    };
    let candidates = onboarding::discover(port, &selected, &ctx).map_err(fail)?;
    Ok((selected, candidates))
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
) -> Result<Candidate, &'static str> {
    let endpoint = if let Some(value) = options.get("--manual-endpoint") {
        value.to_string()
    } else if interactive {
        eprintln!("{}", guidance().join("\n"));
        prompt("Enter the TV's connection IPv4:port from its debugging settings (not the pairing port), or q:")?
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
fn setup_command(options: &Options) -> Result<i32, &'static str> {
    if options.dry && options.get("--fixture").is_none() {
        emit(
            json!({"status":"dry_run","message":"Dry run: no discovery, configuration write or connection performed.","next_steps":["Choose an interface, discover displays, choose a device, preview configuration, then confirm saving.","Use --fixture with --dry-run for an offline full preview."],"network_access":false,"writes":false}),
            options.json,
        );
        return Ok(0);
    }
    let discovery_started = std::time::Instant::now();
    let port = source(options)?;
    let (interface, candidates) = if options.get("--manual-endpoint").is_some() {
        (
            options
                .get("--interface")
                .ok_or("interface_required")?
                .to_string(),
            vec![],
        )
    } else {
        discover(options, &*port)?
    };
    let candidate = if options.get("--manual-endpoint").is_some() || candidates.is_empty() {
        if options.json && options.get("--manual-endpoint").is_none() {
            emit(
                json!({"status":"no_devices_found","message":"No displays advertised. No configuration saved.","next_steps":guidance(),"candidates":[],"interface":interface}),
                true,
            );
            return Ok(2);
        }
        // Bind manual fallback to the interface chosen earlier in this invocation.
        let mut bound = Options {
            values: options.values.clone(),
            ..Options::default()
        };
        bound.values.insert("--interface".into(), interface.clone());
        manual_candidate(&bound, &*port, !options.json)?
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
            |c| format!("{} ({})", c.label, c.address),
            "Choose the display:",
        )?;
        candidates[n].clone()
    };
    let plan = onboarding::plan_setup(candidate, discovery_started.elapsed().as_millis() as u64);
    if plan.endpoint.is_none() {
        emit(
            json!({"status":readiness(&plan.readiness),"message":"Display detected; device setup is required before a control configuration can be saved.","candidate":candidate_json(&plan.candidate),"next_steps":guidance(),"writes":false}),
            options.json,
        );
        return Ok(2);
    }
    let adb = executable(options, "--adb", "adb")?;
    let curl = executable(options, "--curl", "curl")?;
    let server_port: u16 = options
        .get("--server-port")
        .unwrap_or("15037")
        .parse()
        .map_err(|_| "invalid_server_port")?;
    let config = setup::configuration(&plan, adb, curl, server_port)?;
    let mut bytes =
        serde_json::to_vec_pretty(&config).map_err(|_| "configuration_encode_failed")?;
    bytes.push(b'\n');
    let output = PathBuf::from(options.get("--output").unwrap_or("tv-control.local.json"));
    let preview = json!({"status":"confirmation_required","message":"Configuration preview ready. Device control remains unverified.","candidate":candidate_json(&plan.candidate),"configuration":config,"output":output,"simulation":options.get("--fixture").is_some(),"next_steps":guidance(),"writes":false});
    if options.dry || (options.json && !options.confirm) {
        emit(preview, options.json);
        return Ok(0);
    }
    if !options.json {
        eprintln!(
            "ADB executable: {:?}; dedicated loopback server port: {}",
            config.executables.adb, server_port
        );
        eprintln!("Will save {} for {}. No media app or volume calibration is assumed. No device access is granted.",output.display(),plan.endpoint.as_deref().unwrap_or("unknown"));
        if !options.confirm
            && prompt("Save this private configuration? Type yes to confirm:")? != "yes"
        {
            emit(
                json!({"status":"cancelled","message":"Cancelled; no configuration saved.","writes":false}),
                false,
            );
            return Ok(2);
        }
    }
    if plan.candidate.services.iter().any(|s| {
        s.valid_until_monotonic_ms
            .is_some_and(|until| until <= discovery_started.elapsed().as_millis() as u64)
    }) {
        return Err("discovery_expired_rediscover");
    }
    let status = setup::publish(&output, &bytes, options.resume)?;
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
        "discover" => &["--fixture", "--interface", "--duration-ms"],
        _ => &[
            "--fixture",
            "--interface",
            "--duration-ms",
            "--device",
            "--output",
            "--adb",
            "--curl",
            "--server-port",
            "--manual-endpoint",
            "--manual-kind",
        ],
    };
    if options
        .values
        .keys()
        .any(|key| !allowed.contains(&key.as_str()))
        || (options.probe && command != "doctor")
        || ((options.confirm || options.resume) && command != "setup")
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
            let (interface, candidates) = discover(&options, &*port)?;
            if options.json {
                emit(
                    json!({"status":"discovered","interface":interface,"simulation":options.get("--fixture").is_some(),"candidates":candidates.iter().map(candidate_json).collect::<Vec<_>>(),"next_steps":if candidates.is_empty(){guidance()}else{vec!["Choose a candidate with setup; discovery does not verify control or grant access."]}}),
                    true,
                );
            } else {
                println!("{} display candidate(s), all unverified:", candidates.len());
                for c in candidates {
                    println!("  {}  {} ({})", c.id, c.label, c.address);
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
