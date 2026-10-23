//! Human and agent incoming adapters. Setup may acquire approved dependencies,
//! but never executes ADB, authenticates or starts a tunnel.
mod dependencies;
mod discovery;
mod presentation;
mod progress;
mod wizard;

use discovery::{discover_on, discovery_error, select_interface, source};
use presentation::{candidate_json, display_label, emit, guidance, interface_json};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tv_adapters::{
    config::Config,
    runtime::{CancelFlag, SystemClock},
    setup,
};
use tv_application::Clock;
use tv_domain::{Action, Request, Status};
use wizard::setup_command;

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
