//! Passive capture of an existing approved connection; no onboarding or repair.
use super::super::runtime::{private_read_limit, resolve_tunnel, RuntimeIntent};
use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};
use tv_adapters::{
    config::Config,
    dependency_acquisition::file_digest,
    onboarding_tunnel::TunnelSpec,
    pipeline_store::Checkpoint,
    process::{Invocation, Process},
    runtime_credential,
    supervision_store::Binding,
};
use tv_application::Context;
use tv_domain::{Failure, PortResult};

pub(super) struct Existing {
    pub(super) state: PathBuf,
    pub(super) intent: RuntimeIntent,
    pub(super) config: Config,
    pub(super) spec: TunnelSpec,
}
fn fail(code: &'static str) -> Failure {
    Failure::before(code)
}
fn clean(path: &Path) -> PortResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
        || !path
            .to_str()
            .is_some_and(|v| v.len() <= 4096 && !v.chars().any(char::is_control))
    {
        return Err(fail("supervision_path_invalid"));
    }
    Ok(())
}
fn ancestors(path: &Path) -> PortResult<()> {
    clean(path)?;
    for p in path
        .parent()
        .ok_or(fail("supervision_path_invalid"))?
        .ancestors()
    {
        let m = fs::symlink_metadata(p).map_err(|_| fail("supervision_binding_unavailable"))?;
        if !m.is_dir() || m.file_type().is_symlink() {
            return Err(fail("supervision_symlink_refused"));
        }
    }
    Ok(())
}
fn directory(path: &Path, uid: u32) -> PortResult<()> {
    ancestors(path)?;
    let m = fs::symlink_metadata(path).map_err(|_| fail("supervision_binding_unavailable"))?;
    if !m.is_dir() || m.file_type().is_symlink() || m.uid() != uid || m.mode() & 0o7777 != 0o700 {
        return Err(fail("supervision_directory_unsafe"));
    }
    Ok(())
}
#[derive(Clone, Copy)]
enum Kind {
    Private,
    PublicKey,
    Executable,
}
fn metadata(file: &File, uid: u32, kind: Kind) -> PortResult<Metadata> {
    let m = file
        .metadata()
        .map_err(|_| fail("supervision_binding_unavailable"))?;
    let mode = m.mode() & 0o7777;
    let valid = match kind {
        Kind::Private => m.uid() == uid && mode == 0o600,
        Kind::PublicKey => m.uid() == uid && mode & 0o022 == 0 && mode & 0o7000 == 0,
        Kind::Executable => {
            (m.uid() == uid || m.uid() == 0)
                && mode & 0o022 == 0
                && mode & 0o111 != 0
                && mode & 0o7000 == 0
        }
    };
    if !m.is_file() || m.nlink() != 1 || !valid || m.len() == 0 {
        return Err(fail("supervision_file_unsafe"));
    }
    Ok(m)
}
fn same(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.mode() == b.mode()
        && a.uid() == b.uid()
        && a.nlink() == b.nlink()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}
fn pin(
    path: &Path,
    uid: u32,
    kind: Kind,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> PortResult<String> {
    ctx.check()?;
    ancestors(path)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| fail("supervision_binding_unavailable"))?;
    let before = metadata(&file, uid, kind)?;
    let digest = file_digest(process, path, ctx).map_err(fail)?;
    ctx.check()?;
    let after = metadata(&file, uid, kind)?;
    let named = fs::symlink_metadata(path).map_err(|_| fail("supervision_binding_changed"))?;
    if named.file_type().is_symlink() || !same(&before, &after) || !same(&after, &named) {
        return Err(fail("supervision_binding_changed"));
    }
    Ok(digest)
}
fn add(
    pins: &mut BTreeMap<String, String>,
    path: &Path,
    uid: u32,
    kind: Kind,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> PortResult<()> {
    clean(path)?;
    let target = if matches!(kind, Kind::Executable) {
        // Existing executable symlinks (for example Homebrew) are resolved
        // explicitly. Private configuration/identity files never take this path.
        for component in path.ancestors() {
            let m = fs::symlink_metadata(component)
                .map_err(|_| fail("supervision_binding_unavailable"))?;
            if !(m.uid() == uid || m.uid() == 0)
                || (!m.file_type().is_symlink() && m.mode() & 0o022 != 0)
            {
                return Err(fail("supervision_executable_path_unsafe"));
            }
        }
        fs::canonicalize(path).map_err(|_| fail("supervision_binding_unavailable"))?
    } else {
        path.to_owned()
    };
    let digest = pin(&target, uid, kind, process, ctx)?;
    if matches!(kind, Kind::Executable)
        && fs::canonicalize(path).map_err(|_| fail("supervision_binding_changed"))? != target
    {
        return Err(fail("supervision_binding_changed"));
    }
    for path in [path, target.as_path()] {
        let name = path
            .to_str()
            .ok_or(fail("supervision_path_invalid"))?
            .to_owned();
        if pins.get(&name).is_some_and(|old| *old != digest) {
            return Err(fail("supervision_binding_changed"));
        }
        pins.insert(name, digest.clone());
    }
    Ok(())
}
fn checkpoint(configuration: &Path) -> PortResult<(PathBuf, Checkpoint)> {
    ancestors(configuration)?;
    let state = configuration
        .parent()
        .ok_or(fail("supervision_path_invalid"))?
        .join("connection");
    directory(&state, unsafe { libc::geteuid() })?;
    let bytes = private_read_limit(&state.join("progress.json"), 8192).map_err(fail)?;
    let checkpoint: Checkpoint =
        serde_json::from_slice(&bytes).map_err(|_| fail("pipeline_checkpoint_corrupt"))?;
    checkpoint.validate().map_err(fail)?;
    if checkpoint.configuration_path != configuration {
        return Err(fail("supervision_binding_mismatch"));
    }
    Ok((state, checkpoint))
}

pub(super) fn coordinates(
    configuration: &Path,
    ctx: &Context<'_>,
) -> PortResult<(String, PathBuf)> {
    ctx.check()?;
    let (state, saved) = checkpoint(configuration)?;
    let helper = resolve_tunnel(&state, ctx)
        .map_err(fail)?
        .ok_or(fail("tunnel_client_missing"))?;
    Ok((saved.installation_id, helper))
}

pub(super) fn capture(
    configuration: &Path,
    artifact: &Path,
    helper: &Path,
    uid: u32,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> PortResult<Binding> {
    let executable_path = std::env::var("PATH").map_err(|_| fail("executable_path_missing"))?;
    capture_parts(
        configuration,
        artifact,
        helper,
        uid,
        executable_path,
        true,
        process,
        ctx,
    )
    .map(|(binding, _)| binding)
}
#[allow(clippy::too_many_arguments)]
fn capture_parts(
    configuration: &Path,
    artifact: &Path,
    helper: &Path,
    uid: u32,
    executable_path: String,
    initial: bool,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> PortResult<(Binding, Existing)> {
    ctx.check()?;
    if uid == 0 || uid != unsafe { libc::geteuid() } {
        return Err(fail("supervision_uid_mismatch"));
    }
    clean(artifact)?;
    let (state, saved) = checkpoint(configuration)?;
    directory(&state, uid)?;
    let snapshot = private_read_limit(configuration, 1_048_576).map_err(fail)?;
    let config = Config::from_json(&snapshot).map_err(fail)?;
    let intent_path = state.join("runtime-intent.json");
    let intent: RuntimeIntent =
        serde_json::from_slice(&private_read_limit(&intent_path, 6_400_000).map_err(fail)?)
            .map_err(|_| fail("runtime_intent_invalid"))?;
    let tunnel_id = saved
        .tunnel_id
        .as_ref()
        .ok_or(fail("tunnel_configuration_missing"))?;
    let alias = format!("tv-{}", saved.installation_id);
    if !config.generic_adb_control
        || intent.schema != 1
        || intent.installation_id != saved.installation_id
        || intent.revision != saved.configuration_revision
        || intent.device != saved.device
        || intent.configuration.as_bytes() != snapshot
        || saved.tunnel_alias.as_ref() != Some(&alias)
        || !tv_domain::identifier(&intent.revision)
        || !tv_domain::identifier(&intent.device)
    {
        return Err(fail("supervision_binding_mismatch"));
    }
    super::super::runtime::adb_spec(&config, &intent.device, &state).map_err(fail)?;
    let spec = TunnelSpec {
        binary: helper.to_owned(),
        alias,
        tunnel_id: tunnel_id.clone(),
        runtime_key_env: "TV_CONTROL_TUNNEL_KEY".into(),
        state_dir: state.join("tunnel"),
        mcp_binary: intent.controller_binary.clone(),
        configuration: configuration.to_owned(),
        bridge_state_dir: Some(state.clone()),
        executable_path: executable_path.clone().into(),
    };
    spec.validate()?;
    for dir in [
        &spec.state_dir,
        &spec.state_dir.join("profiles"),
        &spec.state_dir.join("home"),
        &state.join("adb"),
        &state.join("adb/adb-home"),
    ] {
        directory(dir, uid)?;
    }
    let owner = spec.state_dir.join(".tv-control-owner");
    let expected = serde_json::to_vec(
        &serde_json::json!({"schema":1,"alias":spec.alias,"tunnel_id":spec.tunnel_id,
        "mcp_command":spec.mcp_command()?,"profile_dir":spec.state_dir.join("profiles")}),
    )
    .map_err(|_| fail("tunnel_binding_encode_failed"))?;
    if private_read_limit(&owner, 65_536).map_err(fail)? != expected {
        return Err(fail("tunnel_binding_changed"));
    }
    if runtime_credential::load(&state, &saved.installation_id, tunnel_id)
        .map_err(fail)?
        .is_none()
    {
        return Err(fail("supervision_saved_credential_required"));
    }
    let mut pins = BTreeMap::new();
    for path in [
        configuration,
        intent_path.as_path(),
        owner.as_path(),
        &state.join("runtime-credential.json"),
        &state.join("adb/adb-home/adbkey"),
    ] {
        add(&mut pins, path, uid, Kind::Private, process, ctx)?;
    }
    add(
        &mut pins,
        &state.join("adb/adb-home/adbkey.pub"),
        uid,
        Kind::PublicKey,
        process,
        ctx,
    )?;
    for path in [
        &intent.controller_binary,
        helper,
        &config.executables.adb,
        &config.executables.curl,
    ] {
        add(&mut pins, path, uid, Kind::Executable, process, ctx)?;
    }
    let payload = std::env::current_exe().map_err(|_| fail("supervision_payload_unavailable"))?;
    if !pins.contains_key(payload.to_str().ok_or(fail("supervision_path_invalid"))?) {
        add(&mut pins, &payload, uid, Kind::Executable, process, ctx)?;
    }
    if initial {
        let output = process.run(
            Invocation {
                program: intent.controller_binary.clone(),
                args: vec!["service".into(), "protocol".into()],
                env: BTreeMap::from([("PATH".into(), executable_path.clone().into())]),
                stdin: Vec::new(),
                mutating: false,
            },
            ctx,
        )?;
        ctx.check()?;
        if !output.success || output.stdout != b"TV_CONTROL_SUPERVISION_V1\n" {
            return Err(fail("supervision_launcher_protocol_unsupported"));
        }
        // The probe must not replace the exact launcher just pinned.
        let mut after = BTreeMap::new();
        add(
            &mut after,
            &intent.controller_binary,
            uid,
            Kind::Executable,
            process,
            ctx,
        )?;
        if after
            .iter()
            .any(|(path, value)| pins.get(path) != Some(value))
        {
            return Err(fail("supervision_binding_changed"));
        }
    }
    ctx.check()?;
    let binding = Binding {
        installation_id: saved.installation_id.clone(),
        tunnel_id: tunnel_id.clone(),
        configuration: configuration.to_owned(),
        launcher: intent.controller_binary.clone(),
        helper: helper.to_owned(),
        executable_path,
        label: format!("com.tv-control.{}", saved.installation_id),
        artifact: artifact.to_owned(),
        uid,
        pins,
    };
    binding.validate()?;
    Ok((
        binding,
        Existing {
            state,
            intent,
            config,
            spec,
        },
    ))
}

pub(super) fn existing(
    binding: &Binding,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> PortResult<Existing> {
    binding.validate()?;
    let (captured, existing) = capture_parts(
        &binding.configuration,
        &binding.artifact,
        &binding.helper,
        binding.uid,
        binding.executable_path.clone(),
        false,
        process,
        ctx,
    )?;
    if captured != *binding {
        return Err(fail("supervision_binding_changed"));
    }
    Ok(existing)
}
