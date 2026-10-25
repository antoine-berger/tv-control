//! Explicit host supervision commands. None is exposed as an MCP TV tool.
mod artifact;
mod commands;
#[cfg(test)]
mod commands_tests;
mod installation_lock;
mod registration_marker;
mod runner;

use crate::{journey::service_runtime, terminal::Interrupts};
use serde_json::{json, Value};
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use tv_adapters::{
    process::BoundedProcess,
    runtime::SystemClock,
    service_platform::ServiceSpec,
    supervision_store::{Binding, SupervisionStore, DIRECTORY},
};
use tv_application::{
    supervision::{Code, Mode, Phase, Record, Registration, Store},
    Clock, Context,
};
use tv_domain::{Failure, PortResult};

pub(crate) const REMOVAL_TOKEN: &str = "TV_CONTROL_SERVICE_REMOVAL_V1_SAFE";

pub(crate) fn connection(configuration: &Path) -> PortResult<PathBuf> {
    let absolute = if configuration.is_absolute() {
        configuration.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| Failure::before("configuration_path_invalid"))?
            .join(configuration)
    };
    let configuration = absolute.as_path();
    let metadata = configuration
        .symlink_metadata()
        .map_err(|_| Failure::before("configuration_unreadable"))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        return Err(Failure::before("supervision_configuration_unsafe"));
    }
    if let Some(parent) = configuration.parent() {
        for ancestor in parent.ancestors() {
            let entry = ancestor
                .symlink_metadata()
                .map_err(|_| Failure::before("supervision_configuration_unsafe"))?;
            if !entry.is_dir() || entry.file_type().is_symlink() {
                return Err(Failure::before("supervision_configuration_unsafe"));
            }
        }
    }
    let path = configuration
        .canonicalize()
        .map_err(|_| Failure::before("configuration_unreadable"))?;
    Ok(path
        .parent()
        .ok_or(Failure::before("configuration_path_invalid"))?
        .join("connection"))
}

fn state_exists(connection: &Path) -> PortResult<bool> {
    match connection.join(DIRECTORY).symlink_metadata() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(Failure::before("supervision_state_unreadable")),
    }
}

fn process() -> BoundedProcess {
    BoundedProcess {
        max_output: 65_536,
        poll_ms: 10,
    }
}

fn spec(binding: &Binding) -> ServiceSpec {
    ServiceSpec {
        label: binding.label.clone(),
        executable: binding.launcher.clone(),
        configuration: binding.configuration.clone(),
        working_directory: binding.configuration.parent().unwrap().join("connection"),
        artifact: binding.artifact.clone(),
        uid: binding.uid,
    }
}

fn status_record(record: &Record) -> Value {
    json!({
        "revision":record.revision,"arm":record.arm,
        "registration":match record.registration {Registration::Prepared=>"prepared",Registration::Installed=>"installed",Registration::Unknown=>"unknown",Registration::Removed=>"removed"},
        "intent":match record.mode {Mode::Stopped=>"stopped",Mode::Enabled=>"enabled",Mode::Blocked=>"blocked"},
        "phase":match record.phase {Phase::Idle=>"idle",Phase::Monitoring=>"monitoring",Phase::StartInFlight=>"start_in_flight",Phase::CleanupInFlight=>"cleanup_in_flight"},
        "controller_starts":record.starts,"unclean_incarnations":record.unclean,
        "code":record.code.as_str(),"runtime_ready_verified":false,
        "note":"Stored lifecycle evidence is not a fresh TV or launchd readiness observation."
    })
}

fn require_platform() -> PortResult<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(Failure::unsupported("service_platform_unsupported"))
    }
}

fn publish_registration(
    store: &SupervisionStore,
    expected: &Record,
    registration: Registration,
    code: Code,
    ctx: &Context<'_>,
) -> PortResult<()> {
    let mut next = expected.clone();
    next.revision = expected
        .revision
        .checked_add(1)
        .ok_or(Failure::before("supervision_revision_exhausted"))?;
    next.registration = registration;
    next.mode = Mode::Stopped;
    next.phase = Phase::Idle;
    next.code = code;
    if store
        .with_context(ctx)
        .compare_and_set(expected.revision, &next)?
    {
        Ok(())
    } else {
        Err(Failure::before("supervision_state_changed"))
    }
}

/// Static installer guard. It never probes, starts or stops a runtime.
pub(crate) fn removal_check(configuration: &Path, launcher: &Path) -> PortResult<()> {
    let connection = connection(configuration)?;
    if !state_exists(&connection)? {
        return Ok(());
    }
    let store = SupervisionStore::open(&connection)?;
    let binding = store.binding()?;
    if binding.launcher != launcher {
        return Ok(());
    }
    let record = store.load()?;
    let artifact_absent = match binding.artifact.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Ok(_) => false,
        Err(_) => return Err(Failure::before("service_artifact_unreadable")),
    };
    if record.registration != Registration::Removed
        || record.mode != Mode::Stopped
        || record.phase != Phase::Idle
        || !artifact_absent
    {
        return Err(Failure::before(
            "service_uninstall_required_installation_preserved",
        ));
    }
    let _owner = store.try_owner()?;
    Ok(())
}

/// Existing disconnect must disarm before it stops the managed runtime. A live
/// supervisor drains through the same explicit stop path; errors preserve access.
pub(crate) fn disarm_disconnect(
    configuration: &Path,
    launcher: &Path,
    ctx: &Context<'_>,
) -> PortResult<bool> {
    let connection = connection(configuration)?;
    if !state_exists(&connection)? {
        return Ok(false);
    }
    let store = SupervisionStore::open_with_context(&connection, ctx)?;
    if store.binding_with_context(ctx)?.launcher != launcher {
        return Err(Failure::before("supervision_launcher_mismatch"));
    }
    commands::stop_store(&store, ctx)?;
    Ok(true)
}

pub(crate) fn run(args: &[String]) -> Result<i32, &'static str> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err("service_command_required");
    };
    if command == "run" {
        if args.len() != 2 {
            return Ok(0);
        }
        return Ok(runner::run(Path::new(&args[1])));
    }
    if command == "uninstall-check" {
        if args.len() != 4 || args[2] != "--launcher" {
            return Err("invalid_service_arguments");
        }
        removal_check(Path::new(&args[1]), Path::new(&args[3])).map_err(|e| e.code)?;
        println!("{REMOVAL_TOKEN}");
        return Ok(0);
    }
    if args.len() < 2 || args.len() > 3 {
        return Err("invalid_service_arguments");
    }
    let approved = args
        .get(2)
        .is_some_and(|value| value == "--approve-persistence");
    if (args.len() == 3 && !approved) || (approved && !matches!(command, "install" | "resume")) {
        return Err("invalid_service_arguments");
    }
    if matches!(command, "install" | "resume") && !approved {
        return Err("service_persistence_approval_required");
    }
    let configuration = Path::new(&args[1]);
    let clock = SystemClock::default();
    let interrupts = Interrupts::install()?;
    let ctx = Context {
        clock: &clock,
        cancellation: &interrupts,
        deadline_monotonic_ms: 60_000,
    };
    let result: PortResult<i32> = (|| match command {
        "plan" => {
            require_platform()?;
            let binding = service_runtime::plan_binding(configuration, &ctx)?;
            let plist = spec(&binding).render()?;
            println!(
                "{}",
                json!({"status":"planned","activated":false,"platform":"macos_user_launch_agent",
                "label":binding.label,"artifact_path":binding.artifact,
                "plist":String::from_utf8(plist).map_err(|_|Failure::before("service_artifact_invalid"))?,
                "opt_in_steps":[[binding.launcher.to_string_lossy().as_ref(),"service","install",binding.configuration.to_string_lossy().as_ref(),"--approve-persistence"],
                    [binding.launcher.to_string_lossy().as_ref(),"service","resume",binding.configuration.to_string_lossy().as_ref(),"--approve-persistence"]],
                "restart_budget_per_arm":3,"unclean_budget_per_arm":3,
                "update_requires_service_uninstall":true})
            );
            Ok(0)
        }
        "status" => {
            let connection = connection(configuration)?;
            if !state_exists(&connection)? {
                println!("{}", json!({"status":"not_configured","activated":false}));
            } else {
                let store = SupervisionStore::open_with_context(&connection, &ctx)?;
                let record = store.load_with_context(&ctx)?;
                let owner_present = match store.try_owner_with_context(&ctx) {
                    Ok(_lease) => false,
                    Err(error) if error.code == "supervision_owner_busy" => true,
                    Err(error) => return Err(error),
                };
                println!(
                    "{}",
                    json!({"status":"recorded","owner_present":owner_present,"state":status_record(&record)})
                );
            }
            Ok(0)
        }
        "install" => commands::install(configuration, &ctx),
        "resume" => commands::resume(configuration, &ctx),
        "stop" => {
            let store = SupervisionStore::open_with_context(&connection(configuration)?, &ctx)?;
            commands::stop_store(&store, &ctx)?;
            println!(
                "{}",
                json!({"status":"stopped","runtime_stopped_verified":true,"identities_preserved":true})
            );
            Ok(0)
        }
        "uninstall" => commands::uninstall(configuration, &ctx),
        _ => Err(Failure::before("unknown_service_command")),
    })();
    result.map_err(|error| error.code)
}
