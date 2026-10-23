mod generic_mcp;
mod journey;
mod journey_fixture;
mod mcp;
mod onboarding;
mod terminal;
use serde_json::{json, Value};
use std::{
    io::{self, Read},
    sync::Arc,
};
use tv_adapters::{
    config::Config,
    http::CurlHttp,
    process::BoundedProcess,
    runtime::{CancelFlag, MemoryState, SystemClock},
    wire, Router,
};
use tv_application::{Clock, Control, Policy, Service};
use tv_domain::{Action, Request, Status};

fn load(path: &str) -> Result<Config, &'static str> {
    let file = std::fs::File::open(path).map_err(|_| "configuration_unreadable")?;
    let mut bytes = Vec::new();
    file.take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| "configuration_unreadable")?;
    if bytes.len() > 1_048_576 {
        return Err("configuration_too_large");
    }
    Config::from_json(&bytes)
}
fn service(cfg: Arc<Config>) -> Arc<dyn Control> {
    let process = Arc::new(BoundedProcess {
        max_output: cfg.preferences.max_output_bytes,
        poll_ms: cfg.preferences.process_poll_ms,
    });
    let http = Arc::new(CurlHttp {
        config: cfg.clone(),
        process: process.clone(),
    });
    let router = Router {
        config: cfg.clone(),
        process,
        http,
        adb_probe: Arc::new(tv_adapters::adb_server::SmartSocketProbe {
            poll_ms: cfg.preferences.process_poll_ms,
        }),
    };
    Arc::new(Service {
        devices: router.clone(),
        catalogue: router,
        state: MemoryState::new(
            cfg.preferences.dedup_entries,
            cfg.preferences.dedup_retention_ms,
        )
        .with_resource_groups(
            cfg.devices
                .iter()
                .map(|(alias, device)| (alias.clone(), device.coordination_group().to_string()))
                .collect(),
        ),
        clock: SystemClock::default(),
        policy: Policy {
            max_budget_ms: cfg.preferences.max_budget_ms,
            verification_poll_ms: cfg.preferences.verification_poll_ms,
            resume_position_tolerance_ms: cfg.preferences.resume_position_tolerance_ms,
        },
    })
}
fn operation_path(config: &str) -> Result<std::path::PathBuf, &'static str> {
    Ok(std::path::Path::new(config)
        .canonicalize()
        .map_err(|_| "configuration_unreadable")?
        .parent()
        .ok_or("configuration_path_invalid")?
        .join(".tv-control-operations"))
}
type McpServices = (Arc<dyn Control>, Arc<dyn tv_application::generic::Control>);
fn mcp_services(cfg: Arc<Config>, path: &std::path::Path) -> Result<McpServices, &'static str> {
    let process = Arc::new(BoundedProcess {
        max_output: 8 * 1024 * 1024,
        poll_ms: cfg.preferences.process_poll_ms,
    });
    let devices = tv_adapters::generic_adb::GenericAdb::new(
        cfg.clone(),
        process,
        Arc::new(tv_adapters::adb_server::SmartSocketProbe {
            poll_ms: cfg.preferences.process_poll_ms,
        }),
    )
    .with_cache_dir(
        path.parent()
            .ok_or("configuration_path_invalid")?
            .join(".tv-control-catalogue"),
    );
    let journal: Arc<dyn tv_application::generic::Journal> =
        Arc::new(tv_adapters::operation_journal::OperationJournal::open(
            path,
            cfg.preferences.dedup_entries,
            cfg.preferences.dedup_retention_ms,
        )?);
    let legacy = Arc::new(tv_application::generic::LegacyGuard::new(
        service(cfg.clone()),
        journal.clone(),
        Arc::new(SystemClock::default()),
        cfg.devices
            .iter()
            .map(|(alias, d)| (alias.clone(), d.coordination_group().to_string()))
            .collect(),
        cfg.preferences.max_budget_ms,
    ));
    let generic = Arc::new(tv_application::generic::Service::new(
        devices,
        journal,
        SystemClock::default(),
        cfg.preferences.max_budget_ms,
        cfg.preferences.verification_poll_ms,
    ));
    Ok((legacy, generic))
}
fn capabilities(cfg: &Config) -> Value {
    json!({"transport":"stdio","protocol_version":"2025-11-25","max_budget_ms":cfg.preferences.max_budget_ms,
        "deduplication":"durable_shared_journal", "generic_adb_control":cfg.generic_adb_control,"captures":false,"shell_tool":false,"network_listener":false,
        "devices":cfg.devices.iter().map(|(alias,d)| match d {
            tv_adapters::config::Device::Adb {media_app,volume_max_index,..}=>json!({"alias":alias,"adapter":"adb","actions":{
                "observe":true,"open_app":true,"pause":cfg.generic_adb_control || media_app.is_some(),"resume":cfg.generic_adb_control || media_app.is_some(),"navigate":cfg.generic_adb_control,"search":"discovered_per_app","ui":"device_dependent","set_volume":volume_max_index.is_some(),"resume_media":false},
                "volume_unit":"android_music_stream_index_requires_calibration"}),
            tv_adapters::config::Device::Jellyfin {..}=>json!({"alias":alias,"adapter":"jellyfin","actions":{
                "observe":true,"open_app":false,"pause":true,"resume":true,"set_volume":false,"resume_media":true},"qualification":"requires_patched_server_and_compatible_client"})
        }).collect::<Vec<_>>(),"apps":cfg.apps.keys().collect::<Vec<_>>(),"media":cfg.media.keys().collect::<Vec<_>>(),"workflows":cfg.workflows.keys().collect::<Vec<_>>()})
}
fn run() -> Result<i32, &'static str> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "onboard")
        && args.len() == 6
        && args[3] == "--fixture"
        && args[5] == "--json"
    {
        return journey_fixture::run(
            std::path::Path::new(&args[2]),
            std::path::Path::new(&args[4]),
        );
    }
    if args.get(1).is_some_and(|s| s == "onboard")
        && (args.len() == 3 || (args.len() == 4 && args[3] == "--json"))
    {
        return journey::run(std::path::Path::new(&args[2]), args.len() == 4, None);
    }
    if args.get(1).is_some_and(|s| s == "onboard") && args.len() == 5 && args[3] == "--launcher" {
        return journey::run(
            std::path::Path::new(&args[2]),
            false,
            Some(std::path::Path::new(&args[4])),
        );
    }
    if args.get(1).is_some_and(|s| s == "bridge") && args.len() == 4 {
        return journey::bridge(
            std::path::Path::new(&args[2]),
            std::path::Path::new(&args[3]),
        );
    }
    if args.get(1).is_some_and(|s| s == "disconnect")
        && (args.len() == 3 || (args.len() == 4 && args[3] == "--json"))
    {
        return journey::disconnect(std::path::Path::new(&args[2]), args.len() == 4, None);
    }
    if args.get(1).is_some_and(|s| s == "disconnect")
        && args.len() == 6
        && args[3] == "--json"
        && args[4] == "--launcher"
    {
        return journey::disconnect(
            std::path::Path::new(&args[2]),
            true,
            Some(std::path::Path::new(&args[5])),
        );
    }
    if args.len() == 2 && matches!(args[1].as_str(), "--help" | "--version") {
        if args[1] == "--version" {
            println!("tv-control {}", env!("CARGO_PKG_VERSION"));
        } else {
            println!("tv-control setup [--json|--dry-run]\ntv-control onboard|disconnect CONFIG [--json]\ntv-control interfaces|discover [--interface NAME] [--json]\ntv-control doctor|connect-chatgpt CONFIG [--json]\ntv-control validate|capabilities|mcp CONFIG\ntv-control execute CONFIG REQUEST.json|-\ntv-control workflow CONFIG ALIAS REQUEST_ID TTL_MS\nSetup prepares configuration. Onboard guides explicit access approvals and verifies the selected conversation. JSON onboarding never grants access.");
        }
        return Ok(0);
    }
    if args.get(1).is_some_and(|s| {
        matches!(
            s.as_str(),
            "interfaces" | "discover" | "setup" | "doctor" | "connect-chatgpt"
        )
    }) {
        return onboarding::run(&args[1], &args[2..]);
    }
    if args.len() < 3 {
        return Err("usage_requires_command_and_configuration");
    }
    let cfg = Arc::new(load(&args[2])?);
    match args[1].as_str() {
        "validate" if args.len() == 3 => {
            println!("{{\"valid\":true}}");
            Ok(0)
        }
        "capabilities" if args.len() == 3 => {
            println!("{}", capabilities(&cfg));
            Ok(0)
        }
        "mcp" if args.len() == 3 => {
            let state = operation_path(&args[2])?;
            let (legacy, generic) = mcp_services(cfg.clone(), &state)?;
            mcp::serve(legacy, generic, cfg)?;
            Ok(0)
        }
        "execute" if args.len() == 4 => {
            let input: Box<dyn Read> = if args[3] == "-" {
                Box::new(io::stdin())
            } else {
                Box::new(std::fs::File::open(&args[3]).map_err(|_| "request_unreadable")?)
            };
            let mut bytes = Vec::new();
            input
                .take(cfg.preferences.max_frame_bytes as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "request_unreadable")?;
            if bytes.len() > cfg.preferences.max_frame_bytes {
                return Err("request_too_large");
            }
            let mut value: Value =
                serde_json::from_slice(&bytes).map_err(|_| "invalid_request_json")?;
            let (legacy, generic) = mcp_services(cfg.clone(), &operation_path(&args[2])?)?;
            if generic_mcp::routes_generic(&cfg, "tv_control", &value) {
                generic_mcp::translate_alias(&cfg, &mut value);
                let call = generic_mcp::Call::parse("tv_control", value)?;
                let response = call.execute(&*generic, &CancelFlag::default());
                println!("{}", response.value);
                return Ok(if response.failed { 2 } else { 0 });
            }
            let request: wire::WireRequest =
                serde_json::from_value(value).map_err(|_| "invalid_request_json")?;
            let result = legacy.execute(request.into(), &CancelFlag::default());
            println!("{}", wire::receipt(&result));
            Ok(
                if matches!(result.status, Status::Verified | Status::Observed) {
                    0
                } else {
                    2
                },
            )
        }
        "workflow" if args.len() == 6 => {
            let workflow = cfg.workflows.get(&args[3]).ok_or("unknown_workflow")?;
            let ttl: u64 = args[5].parse().map_err(|_| "invalid_ttl")?;
            let req = Request {
                id: args[4].clone(),
                device: workflow.device.clone(),
                action: Action::ResumeMedia {
                    media: workflow.media.clone(),
                },
                expires_at_unix_ms: SystemClock::default()
                    .unix_ms()
                    .checked_add(ttl)
                    .ok_or("invalid_ttl")?,
            };
            let (legacy, _) = mcp_services(cfg, &operation_path(&args[2])?)?;
            let result = legacy.execute(req, &CancelFlag::default());
            println!("{}", wire::receipt(&result));
            Ok(if result.status == Status::Verified {
                0
            } else {
                2
            })
        }
        _ => Err("unknown_command_or_invalid_arguments"),
    }
}
fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(code) => {
            if std::env::args().any(|s| s == "--json") {
                println!("{}", json!({"status":"error","code":code}));
            } else {
                eprintln!("tv-control: {code}");
            }
            std::process::exit(1);
        }
    }
}
