//! Official tunnel-client lifecycle adapter. Health is not conversation exposure.
//! The only persistent runtime operation is the reviewed native `runtimes` API.
mod mcp_health;

use crate::{
    process::{Invocation, Process},
    setup::shell_quote,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::{self, DirBuilder, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
use tv_application::Context;
use tv_domain::{Failure, PortResult};

pub struct TunnelSpec {
    pub binary: PathBuf,
    pub alias: String,
    pub tunnel_id: String,
    pub runtime_key_env: String,
    /// A dedicated child of an already private, caller-owned directory.
    pub state_dir: PathBuf,
    pub mcp_binary: PathBuf,
    pub configuration: PathBuf,
    /// When supplied, the scoped bridge owns ADB and MCP under the official supervisor.
    pub bridge_state_dir: Option<PathBuf>,
    /// Only these explicit executable search directories reach the official client.
    pub executable_path: OsString,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeHealth {
    pub alias: String,
    pub tunnel_id: String,
    pub process_running: bool,
    pub healthy: bool,
    pub ready: bool,
    /// A bounded diagnostic from passive MCP observations, never a raw error.
    pub mcp_failure_code: Option<&'static str>,
}
impl RuntimeHealth {
    pub fn locally_ready(&self) -> bool {
        self.process_running && self.healthy && self.ready && self.mcp_failure_code.is_none()
    }
}

pub struct OfficialTunnel<'a> {
    pub process: &'a dyn Process,
}

fn before(code: &'static str) -> Failure {
    Failure::before(code)
}
fn clean_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
}
fn private_directory(path: &Path) -> PortResult<()> {
    let m =
        fs::symlink_metadata(path).map_err(|_| before("tunnel_private_directory_unavailable"))?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.permissions().mode() & 0o077 != 0 {
        return Err(before("tunnel_directory_not_private"));
    }
    Ok(())
}
fn create_directory(path: &Path) -> PortResult<()> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => private_directory(path),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => private_directory(path),
        Err(_) => Err(before("tunnel_directory_creation_failed")),
    }
}
impl TunnelSpec {
    pub fn validate(&self) -> PortResult<()> {
        let valid_alias = !self.alias.is_empty()
            && self.alias.len() <= 64
            && self
                .alias
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
        let id = self
            .tunnel_id
            .strip_prefix("tunnel_")
            .ok_or(before("invalid_tunnel_id"))?;
        let valid_id = if id.len() == 32 {
            id.bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        } else if id.len() == 37 && id.as_bytes()[4] == b'_' {
            id.as_bytes()[..4]
                .iter()
                .chain(id.as_bytes()[5..].iter())
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        } else {
            false
        };
        let key = &self.runtime_key_env;
        let valid_key = key.starts_with("TV_CONTROL_")
            && key.len() <= 96
            && key
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_');
        let valid_path = self.executable_path.to_str().is_some_and(|s| {
            !s.is_empty()
                && s.split(':')
                    .all(|part| !part.is_empty() && clean_path(Path::new(part)))
        });
        if !valid_alias
            || !valid_id
            || !valid_key
            || !valid_path
            || ![
                &self.binary,
                &self.state_dir,
                &self.mcp_binary,
                &self.configuration,
            ]
            .iter()
            .all(|p| clean_path(p))
            || self
                .bridge_state_dir
                .as_ref()
                .is_some_and(|p| !clean_path(p))
        {
            return Err(before("invalid_tunnel_specification"));
        }
        Ok(())
    }
    pub fn mcp_command(&self) -> PortResult<String> {
        self.validate()?;
        let binary = shell_quote(self.mcp_binary.to_str().unwrap()).map_err(before)?;
        let configuration = shell_quote(self.configuration.to_str().unwrap()).map_err(before)?;
        match &self.bridge_state_dir {
            Some(state) => Ok(format!(
                "{binary} bridge {configuration} {}",
                shell_quote(state.to_str().unwrap()).map_err(before)?
            )),
            None => Ok(format!("{binary} mcp {configuration}")),
        }
    }
    fn profile_dir(&self) -> PathBuf {
        self.state_dir.join("profiles")
    }
    fn marker(&self) -> PortResult<Vec<u8>> {
        serde_json::to_vec(
            &json!({"schema":1,"alias":self.alias,"tunnel_id":self.tunnel_id,
            "mcp_command":self.mcp_command()?,"profile_dir":self.profile_dir()}),
        )
        .map_err(|_| before("tunnel_binding_encode_failed"))
    }
    /// The caller must hold its installation/session lock across this adapter's calls.
    fn ownership(&self, create: bool) -> PortResult<()> {
        self.validate()?;
        let parent = self
            .state_dir
            .parent()
            .ok_or(before("invalid_tunnel_specification"))?;
        private_directory(parent)?;
        if !self.state_dir.exists() {
            if !create {
                return Err(before("tunnel_state_missing"));
            }
            create_directory(&self.state_dir)?;
        }
        private_directory(&self.state_dir)?;
        let marker = self.state_dir.join(".tv-control-owner");
        let pending = self.state_dir.join(".tv-control-owner.pending");
        let expected = self.marker()?;
        if !marker.exists() && create {
            // Refuse to adopt a directory containing a previous/foreign runtime.
            for entry in
                fs::read_dir(&self.state_dir).map_err(|_| before("tunnel_state_unreadable"))?
            {
                let entry = entry.map_err(|_| before("tunnel_state_unreadable"))?;
                if entry.path() != pending {
                    return Err(before("tunnel_state_not_owned"));
                }
                let mut file = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(&pending)
                    .map_err(|_| before("tunnel_state_not_owned"))?;
                let metadata = file
                    .metadata()
                    .map_err(|_| before("tunnel_state_unreadable"))?;
                if !metadata.is_file()
                    || metadata.uid() != unsafe { libc::geteuid() }
                    || metadata.permissions().mode() & 0o077 != 0
                    || metadata.nlink() != 1
                {
                    return Err(before("tunnel_owner_recovery_required"));
                }
                let mut contents = Vec::new();
                Read::by_ref(&mut file)
                    .take(65537)
                    .read_to_end(&mut contents)
                    .map_err(|_| before("tunnel_state_unreadable"))?;
                if contents != expected {
                    return Err(before("tunnel_owner_recovery_required"));
                }
            }
            if !pending.exists() {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&pending)
                    .map_err(|_| before("tunnel_owner_creation_failed"))?;
                file.write_all(&expected)
                    .and_then(|_| file.sync_all())
                    .map_err(|_| before("tunnel_owner_creation_failed"))?;
            }
            // Publication refuses an existing marker; an interrupted link remains verifiable.
            fs::hard_link(&pending, &marker).map_err(|_| before("tunnel_owner_creation_failed"))?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&marker)
            .map_err(|_| before("tunnel_state_not_owned"))?;
        let m = file
            .metadata()
            .map_err(|_| before("tunnel_state_unreadable"))?;
        if !m.is_file()
            || m.uid() != unsafe { libc::geteuid() }
            || m.permissions().mode() & 0o077 != 0
            || !(1..=2).contains(&m.nlink())
        {
            return Err(before("tunnel_state_not_owned"));
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|_| before("tunnel_state_unreadable"))?;
        if bytes != expected {
            return Err(before("tunnel_binding_changed"));
        }
        if m.nlink() == 2 {
            let temp = fs::symlink_metadata(&pending)
                .map_err(|_| before("tunnel_owner_recovery_required"))?;
            if !temp.is_file()
                || temp.dev() != m.dev()
                || temp.ino() != m.ino()
                || temp.nlink() != 2
            {
                return Err(before("tunnel_owner_recovery_required"));
            }
            fs::remove_file(&pending).map_err(|_| before("tunnel_owner_recovery_required"))?;
        } else if pending.symlink_metadata().is_ok() {
            return Err(before("tunnel_owner_recovery_required"));
        }
        if create {
            create_directory(&self.profile_dir())?;
            create_directory(&self.state_dir.join("home"))?;
        }
        Ok(())
    }
}

impl OfficialTunnel<'_> {
    fn invocation(
        &self,
        spec: &TunnelSpec,
        args: Vec<OsString>,
        key: Option<&OsStr>,
        mutating: bool,
    ) -> Invocation {
        let mut env = BTreeMap::from([
            ("HOME".into(), spec.state_dir.join("home").into_os_string()),
            ("PATH".into(), spec.executable_path.clone()),
            (
                "TUNNEL_CLIENT_STATE_DIR".into(),
                spec.state_dir.clone().into_os_string(),
            ),
        ]);
        if let Some(key) = key {
            env.insert(spec.runtime_key_env.clone().into(), key.into());
        }
        Invocation {
            program: spec.binary.clone(),
            args,
            env,
            stdin: Vec::new(),
            mutating,
        }
    }
    fn run_json(
        &self,
        spec: &TunnelSpec,
        args: Vec<OsString>,
        key: Option<&OsStr>,
        mutating: bool,
        ctx: &Context<'_>,
    ) -> PortResult<Value> {
        ctx.check()?;
        let output = self
            .process
            .run(self.invocation(spec, args, key, mutating), ctx)?;
        ctx.check().map_err(|error| {
            if mutating {
                error.after_dispatch()
            } else {
                error
            }
        })?;
        if !output.success {
            return Err(if mutating {
                Failure::uncertain("tunnel_operation_failed")
            } else {
                before("tunnel_status_failed")
            });
        }
        serde_json::from_slice(&output.stdout).map_err(|_| {
            if mutating {
                Failure::uncertain("tunnel_invalid_json")
            } else {
                before("tunnel_invalid_json")
            }
        })
    }
    fn parse_status(&self, spec: &TunnelSpec, value: &Value) -> PortResult<RuntimeHealth> {
        let profile_dir = spec.profile_dir();
        let expected_command = spec.mcp_command()?;
        if value.get("alias") != Some(&json!(spec.alias))
            || value.get("tunnel_id") != Some(&json!(spec.tunnel_id))
            || value.get("profile_dir") != Some(&json!(profile_dir))
            || value.get("profile_name") != Some(&json!(spec.alias))
            || value.get("stale").and_then(Value::as_bool) != Some(false)
        {
            return Err(before("tunnel_runtime_identity_mismatch"));
        }
        let flag = |name| {
            value
                .get(name)
                .and_then(Value::as_bool)
                .ok_or(before("tunnel_invalid_status"))
        };
        let running = flag("process_running")?;
        if (!value.get("process").is_some_and(Value::is_null) || running)
            && (value.pointer("/process/alias") != Some(&json!(spec.alias))
                || value.pointer("/process/tunnel_id") != Some(&json!(spec.tunnel_id))
                || value.pointer("/process/profile_dir") != Some(&json!(profile_dir))
                || value.pointer("/process/profile_name") != Some(&json!(spec.alias))
                || value.pointer("/process/target_kind") != Some(&json!("command"))
                || value.pointer("/process/target_value") != Some(&json!(expected_command)))
        {
            return Err(before("tunnel_runtime_identity_mismatch"));
        }
        let healthy = flag("healthy")?;
        let mut ready = flag("ready")?;
        if value
            .pointer("/control_plane_poll_health/state")
            .and_then(Value::as_str)
            .is_some_and(|s| !matches!(s, "healthy" | "direct" | "unknown" | ""))
        {
            ready = false;
        }
        Ok(RuntimeHealth {
            alias: spec.alias.clone(),
            tunnel_id: spec.tunnel_id.clone(),
            process_running: running,
            healthy,
            ready,
            mcp_failure_code: None,
        })
    }
    pub fn status(&self, spec: &TunnelSpec, ctx: &Context<'_>) -> PortResult<RuntimeHealth> {
        self.status_inner(spec, ctx, true)
    }
    fn status_inner(
        &self,
        spec: &TunnelSpec,
        ctx: &Context<'_>,
        observe_mcp: bool,
    ) -> PortResult<RuntimeHealth> {
        spec.ownership(false)?;
        let value = self.run_json(
            spec,
            vec![
                "runtimes".into(),
                "status".into(),
                spec.alias.clone().into(),
                "--json".into(),
            ],
            None,
            false,
            ctx,
        )?;
        let mut health = self.parse_status(spec, &value)?;
        // The official CLI advertises this link only after confirming support;
        // older runtimes omit it. Its ready/healthy flags cover startup/liveness,
        // so an observed MCP failure must additionally gate our local readiness.
        // An unobserved child is allowed: requiring discovery here would prevent
        // the user from reaching the plugin UI that initiates first discovery.
        if observe_mcp && health.process_running {
            if let Some(url) = value.get("mcp_health_url") {
                let url = url
                    .as_str()
                    .ok_or(before("tunnel_mcp_health_invalid_url"))?;
                health.mcp_failure_code = mcp_health::failure_code(url, ctx)?;
                health.ready &= health.mcp_failure_code.is_none();
            }
        }
        Ok(health)
    }
    /// Explicit approval covers local state/profile creation and persistent runtime connection.
    /// It never creates a remote tunnel and never receives an administrator credential.
    pub fn ensure(
        &self,
        spec: &TunnelSpec,
        approved: bool,
        runtime_key: Option<&OsStr>,
        ctx: &Context<'_>,
    ) -> PortResult<RuntimeHealth> {
        ctx.check()?;
        if !approved {
            return Err(before("tunnel_approval_required"));
        }
        spec.ownership(true)?;
        let listed = self.run_json(
            spec,
            vec!["runtimes".into(), "list".into(), "--json".into()],
            None,
            false,
            ctx,
        )?;
        let aliases = listed
            .get("aliases")
            .and_then(Value::as_array)
            .ok_or(before("tunnel_invalid_status"))?;
        if aliases.len() > 1
            || aliases.iter().any(|v| {
                v.get("alias") != Some(&json!(spec.alias))
                    || v.get("tunnel_id") != Some(&json!(spec.tunnel_id))
                    || v.get("profile_dir") != Some(&json!(spec.profile_dir()))
                    || v.get("profile_name") != Some(&json!(spec.alias))
            })
        {
            return Err(before("tunnel_alias_collision"));
        }
        if !aliases.is_empty() {
            let health = self.status(spec, ctx)?;
            if health.process_running {
                return if health.locally_ready() {
                    Ok(health)
                } else {
                    Err(before(
                        health
                            .mcp_failure_code
                            .unwrap_or("tunnel_runtime_not_ready"),
                    ))
                };
            }
        }
        let key = runtime_key
            .filter(|s| !s.is_empty())
            .ok_or(before("tunnel_runtime_key_required"))?;
        let args = vec![
            "runtimes".into(),
            "connect".into(),
            "--alias".into(),
            spec.alias.clone().into(),
            "--tunnel-id".into(),
            spec.tunnel_id.clone().into(),
            "--runtime-api-key".into(),
            format!("env:{}", spec.runtime_key_env).into(),
            "--mcp-command".into(),
            spec.mcp_command()?.into(),
            "--profile-dir".into(),
            spec.profile_dir().into_os_string(),
            "--profile".into(),
            spec.alias.clone().into(),
            "--json".into(),
        ];
        self.run_json(spec, args, Some(key), true, ctx)?;
        let health = self
            .status(spec, ctx)
            .map_err(|error| error.after_dispatch())?;
        if !health.locally_ready() {
            return Err(Failure::uncertain(
                health
                    .mcp_failure_code
                    .unwrap_or("tunnel_runtime_not_ready"),
            ));
        }
        Ok(health)
    }
    pub fn stop(
        &self,
        spec: &TunnelSpec,
        approved: bool,
        ctx: &Context<'_>,
    ) -> PortResult<RuntimeHealth> {
        ctx.check()?;
        if !approved {
            return Err(before("tunnel_stop_approval_required"));
        }
        // Optional diagnostics must not prevent cleanup of an identity-verified
        // owned runtime, including malformed/unreachable health endpoints.
        let health = self.status_inner(spec, ctx, false)?;
        if !health.process_running {
            return Ok(health);
        }
        let value = self.run_json(
            spec,
            vec![
                "runtimes".into(),
                "stop".into(),
                spec.alias.clone().into(),
                "--json".into(),
            ],
            None,
            true,
            ctx,
        )?;
        let stopped = self
            .parse_status(spec, &value)
            .map_err(|error| error.after_dispatch())?;
        if stopped.process_running {
            return Err(Failure::uncertain("tunnel_stop_unverified"));
        }
        Ok(stopped)
    }
}
