//! Private atomic configuration publication. No network or subprocess operations.
use crate::config::{Config, Device};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tv_domain::onboarding::SetupPlan;
pub fn configuration(
    plan: &SetupPlan,
    adb: PathBuf,
    curl: PathBuf,
    server_port: u16,
) -> Result<Config, &'static str> {
    let endpoint = plan.endpoint.clone().ok_or("device_setup_required")?;
    let mut config = Config::from_json(include_bytes!("../../../examples/config.json"))?;
    config.executables.adb = adb;
    config.executables.curl = curl;
    config.devices.clear();
    config.apps.clear();
    config.servers.clear();
    config.media.clear();
    config.workflows.clear();
    let service =
        tv_application::onboarding::connection_service(plan).and_then(|s| s.instance.clone());
    config.devices.insert(
        "screen".into(),
        Device::Adb {
            display_name: None,
            coordination_group: "screen".into(),
            endpoint,
            server_host: "127.0.0.1".into(),
            server_port,
            media_app: None,
            volume_max_index: None,
            discovery_service: service,
        },
    );
    config.validate()?;
    Ok(config)
}
pub fn read_bounded(path: &Path) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "file_unreadable")?
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| "file_unreadable")?;
    if bytes.len() > 1_048_576 {
        return Err("file_too_large");
    }
    Ok(bytes)
}
pub fn publish(path: &Path, bytes: &[u8], resume: bool) -> Result<&'static str, &'static str> {
    if bytes.len() > 1_048_576 {
        return Err("configuration_too_large");
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = parent
        .canonicalize()
        .map_err(|_| "output_directory_missing")?;
    let name = path.file_name().ok_or("invalid_output_path")?;
    let target = parent.join(name);
    if target.symlink_metadata().is_ok() {
        if resume {
            use std::os::unix::fs::PermissionsExt;
            let metadata = target
                .symlink_metadata()
                .map_err(|_| "existing_config_unreadable")?;
            if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
                return Err("existing_config_not_private");
            }
            if read_bounded(&target)? == bytes {
                return Ok("already_configured");
            }
            return Err("existing_config_differs_use_new_output");
        }
        return Err("output_exists_use_resume_or_new_path");
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "clock_unavailable")?
        .as_nanos();
    let temporary = parent.join(format!(".tv-control-{}-{stamp}.tmp", std::process::id()));
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)
        .map_err(|_| "temporary_config_failed")?;
    let _cleanup = Cleanup(temporary.clone());
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "configuration_write_failed")?;
    // Hard-link publication is atomic and refuses an existing destination, including symlinks.
    fs::hard_link(&temporary, &target).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            "output_exists_use_resume_or_new_path"
        } else {
            "configuration_publish_failed"
        }
    })?;
    if let Ok(directory) = fs::File::open(&parent) {
        let _ = directory.sync_all();
    }
    Ok("configuration_saved")
}
/// POSIX shell quoting for displayed commands and tunnel-client's stdio command string.
pub fn shell_quote(value: &str) -> Result<String, &'static str> {
    if value.chars().any(|c| c.is_control()) {
        return Err("command_contains_control_character");
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}
