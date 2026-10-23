use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    ffi::OsStr,
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
use tv_adapters::{
    onboarding_tunnel::{OfficialTunnel, TunnelSpec},
    process::{Invocation, Output, Process},
    runtime::{CancelFlag, SystemClock},
};
use tv_application::{Clock, Context};
use tv_domain::{Effect, PortResult};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "tv-tunnel-contract-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    fn spec(&self) -> TunnelSpec {
        TunnelSpec {
            binary: "/fixtures/tunnel-client".into(),
            alias: "tv-fixture".into(),
            tunnel_id: "tunnel_0123456789abcdef0123456789abcdef".into(),
            runtime_key_env: "TV_CONTROL_TUNNEL_KEY".into(),
            state_dir: self.0.join("tunnel"),
            mcp_binary: self.0.join("program's folder/tv-control"),
            configuration: self.0.join("configuration's folder/tv.json"),
            bridge_state_dir: None,
            executable_path: "/usr/bin:/bin".into(),
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Mock {
    replies: Mutex<VecDeque<(bool, Value)>>,
    invocations: Mutex<Vec<Invocation>>,
}
impl Mock {
    fn new(values: Vec<Value>) -> Self {
        Self {
            replies: Mutex::new(values.into_iter().map(|v| (true, v)).collect()),
            invocations: Mutex::new(Vec::new()),
        }
    }
}
impl Process for Mock {
    fn run(&self, invocation: Invocation, _: &Context<'_>) -> PortResult<Output> {
        self.invocations.lock().unwrap().push(invocation);
        let (success, value) = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected subprocess");
        Ok(Output {
            stdout: serde_json::to_vec(&value).unwrap(),
            success,
        })
    }
}
fn status(spec: &TunnelSpec, running: bool) -> Value {
    json!({"alias":spec.alias,"tunnel_id":spec.tunnel_id,"profile_name":spec.alias,"profile_dir":spec.state_dir.join("profiles"),
        "stale":false,"process_running":running,"healthy":running,"ready":running,
        "process":{"alias":spec.alias,"tunnel_id":spec.tunnel_id,"profile_name":spec.alias,"profile_dir":spec.state_dir.join("profiles"),"target_kind":"command","target_value":spec.mcp_command().unwrap()}})
}
fn aliases(spec: &TunnelSpec) -> Value {
    json!({"aliases":[{"alias":spec.alias,"tunnel_id":spec.tunnel_id,"profile_name":spec.alias,"profile_dir":spec.state_dir.join("profiles")}]})
}
fn context<'a>(clock: &'a SystemClock, cancel: &'a CancelFlag) -> Context<'a> {
    Context {
        clock,
        cancellation: cancel,
        deadline_monotonic_ms: clock.monotonic_ms() + 5000,
    }
}
fn establish(spec: &TunnelSpec) {
    let mock = Mock::new(vec![
        json!({"aliases":[]}),
        json!({"launched":true}),
        status(spec, true),
    ]);
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    OfficialTunnel { process: &mock }
        .ensure(
            spec,
            true,
            Some(OsStr::new("fixture-runtime-value")),
            &context(&clock, &cancel),
        )
        .unwrap();
}

#[test]
fn no_approval_means_no_files_no_subprocess_and_no_implicit_admin_access() {
    let directory = Directory::new();
    let spec = directory.spec();
    let mock = Mock::new(vec![]);
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    let adapter = OfficialTunnel { process: &mock };
    assert_eq!(
        adapter.ensure(&spec, false, None, &ctx).unwrap_err().code,
        "tunnel_approval_required"
    );
    assert_eq!(
        adapter.stop(&spec, false, &ctx).unwrap_err().code,
        "tunnel_stop_approval_required"
    );
    assert!(!spec.state_dir.exists());
    assert!(mock.invocations.lock().unwrap().is_empty());
}
#[test]
fn bridge_command_is_typed_quoted_and_bound_to_its_private_state() {
    let directory = Directory::new();
    let mut spec = directory.spec();
    spec.bridge_state_dir = Some(directory.0.join("bridge's state"));
    let command = spec.mcp_command().unwrap();
    assert!(command.contains(" bridge "));
    assert!(command.contains("bridge'\\''s state"));
    assert!(!command.contains(" mcp "));
    establish(&spec);
    spec.bridge_state_dir = Some(directory.0.join("other-bridge"));
    let mock = Mock::new(vec![]);
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    assert_eq!(
        OfficialTunnel { process: &mock }
            .status(&spec, &context(&clock, &cancel))
            .unwrap_err()
            .code,
        "tunnel_binding_changed"
    );
    assert!(mock.invocations.lock().unwrap().is_empty());
}
#[test]
fn native_connect_uses_secret_reference_isolated_state_and_independent_status() {
    let directory = Directory::new();
    let spec = directory.spec();
    let mock = Mock::new(vec![
        json!({"aliases":[]}),
        json!({"launched":true}),
        status(&spec, true),
    ]);
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let result = OfficialTunnel { process: &mock }
        .ensure(
            &spec,
            true,
            Some(OsStr::new("fixture-runtime-value")),
            &context(&clock, &cancel),
        )
        .unwrap();
    assert!(result.locally_ready());
    let calls = mock.invocations.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].args, ["runtimes", "list", "--json"]);
    assert_eq!(
        calls[2].args,
        ["runtimes", "status", spec.alias.as_str(), "--json"]
    );
    assert!(calls[1].mutating);
    assert!(!calls[0].mutating);
    assert!(!calls[2].mutating);
    let args = calls[1]
        .args
        .iter()
        .map(|a| a.to_str().unwrap())
        .collect::<Vec<_>>();
    assert!(args
        .windows(2)
        .any(|a| a == ["--runtime-api-key", "env:TV_CONTROL_TUNNEL_KEY"]));
    assert!(args
        .windows(2)
        .any(|a| a == ["--tunnel-id", spec.tunnel_id.as_str()]));
    assert!(!args
        .iter()
        .any(|a| a.contains("fixture-runtime-value") || a.contains("admin") || a == &"create"));
    assert_eq!(
        calls[1].env.get(OsStr::new("TV_CONTROL_TUNNEL_KEY")),
        Some(&"fixture-runtime-value".into())
    );
    assert_eq!(calls[1].env.len(), 4);
    assert_eq!(calls[0].env.len(), 3);
    assert_eq!(calls[2].env.len(), 3);
    assert_eq!(
        calls[1].env.get(OsStr::new("TUNNEL_CLIENT_STATE_DIR")),
        Some(&spec.state_dir.clone().into_os_string())
    );
    let command = spec.mcp_command().unwrap();
    assert!(command.contains("'\\''"));
    assert!(args
        .windows(2)
        .any(|a| a == ["--mcp-command", command.as_str()]));
    assert_eq!(
        fs::metadata(&spec.state_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let marker = fs::read(spec.state_dir.join(".tv-control-owner")).unwrap();
    assert!(!String::from_utf8(marker)
        .unwrap()
        .contains("fixture-runtime-value"));
}
#[test]
fn an_owned_healthy_runtime_can_resume_without_reading_a_new_key() {
    let directory = Directory::new();
    let spec = directory.spec();
    establish(&spec);
    let mock = Mock::new(vec![aliases(&spec), status(&spec, true)]);
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    assert!(OfficialTunnel { process: &mock }
        .ensure(&spec, true, None, &context(&clock, &cancel))
        .unwrap()
        .locally_ready());
    assert_eq!(mock.invocations.lock().unwrap().len(), 2);
}
#[test]
fn aliases_and_actual_mcp_target_must_match_before_any_reconnect_or_stop() {
    let directory = Directory::new();
    let spec = directory.spec();
    establish(&spec);
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    let mut wrong_alias = aliases(&spec);
    wrong_alias["aliases"][0]["tunnel_id"] = json!("tunnel_ffffffffffffffffffffffffffffffff");
    let mock = Mock::new(vec![wrong_alias]);
    assert_eq!(
        OfficialTunnel { process: &mock }
            .ensure(&spec, true, None, &ctx)
            .unwrap_err()
            .code,
        "tunnel_alias_collision"
    );
    assert_eq!(mock.invocations.lock().unwrap().len(), 1);
    let mut wrong_target = status(&spec, true);
    wrong_target["process"]["target_value"] = json!("different program");
    let mock = Mock::new(vec![wrong_target]);
    assert_eq!(
        OfficialTunnel { process: &mock }
            .stop(&spec, true, &ctx)
            .unwrap_err()
            .code,
        "tunnel_runtime_identity_mismatch"
    );
    assert_eq!(mock.invocations.lock().unwrap().len(), 1);
}
#[test]
fn dispatch_ack_health_strings_and_poll_failure_do_not_establish_readiness() {
    let directory = Directory::new();
    let spec = directory.spec();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    let mock = Mock::new(vec![
        json!({"aliases":[]}),
        json!({"launched":true}),
        status(&spec, false),
    ]);
    assert_eq!(
        OfficialTunnel { process: &mock }
            .ensure(&spec, true, Some(OsStr::new("fixture-value")), &ctx)
            .unwrap_err()
            .code,
        "tunnel_runtime_not_ready"
    );
    for (field, value) in [("healthy", json!("true")), ("ready", json!(null))] {
        let mut response = status(&spec, true);
        response[field] = value;
        let mock = Mock::new(vec![response]);
        assert_eq!(
            OfficialTunnel { process: &mock }
                .status(&spec, &ctx)
                .unwrap_err()
                .code,
            "tunnel_invalid_status"
        );
    }
    let mut response = status(&spec, true);
    response["control_plane_poll_health"] = json!({"state":"unhealthy"});
    let mock = Mock::new(vec![response]);
    assert!(!OfficialTunnel { process: &mock }
        .status(&spec, &ctx)
        .unwrap()
        .locally_ready());
}
#[test]
fn foreign_state_and_changed_binding_are_preserved() {
    let directory = Directory::new();
    let mut spec = directory.spec();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&spec.state_dir)
        .unwrap();
    fs::write(spec.state_dir.join("foreign"), b"preserve").unwrap();
    let mock = Mock::new(vec![]);
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    assert_eq!(
        OfficialTunnel { process: &mock }
            .ensure(&spec, true, None, &ctx)
            .unwrap_err()
            .code,
        "tunnel_state_not_owned"
    );
    assert_eq!(
        fs::read(spec.state_dir.join("foreign")).unwrap(),
        b"preserve"
    );
    fs::remove_file(spec.state_dir.join("foreign")).unwrap();
    establish(&spec);
    spec.configuration = directory.0.join("different-config.json");
    assert_eq!(
        OfficialTunnel { process: &mock }
            .ensure(&spec, true, None, &ctx)
            .unwrap_err()
            .code,
        "tunnel_binding_changed"
    );
    assert!(mock.invocations.lock().unwrap().is_empty());
}
#[test]
fn bounded_failures_remain_uncertain_after_connect_and_stop_is_verified() {
    let directory = Directory::new();
    let spec = directory.spec();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    let mock = Mock {
        replies: Mutex::new(VecDeque::from([
            (true, json!({"aliases":[]})),
            (false, json!({"error":"fixture"})),
        ])),
        invocations: Mutex::new(Vec::new()),
    };
    let error = OfficialTunnel { process: &mock }
        .ensure(&spec, true, Some(OsStr::new("fixture-value")), &ctx)
        .unwrap_err();
    assert_eq!(error.code, "tunnel_operation_failed");
    assert_eq!(error.effect, Effect::MayHaveChanged);
    let mock = Mock::new(vec![status(&spec, true), status(&spec, false)]);
    assert!(
        !OfficialTunnel { process: &mock }
            .stop(&spec, true, &ctx)
            .unwrap()
            .process_running
    );
    assert_eq!(
        mock.invocations.lock().unwrap()[1].args,
        ["runtimes", "stop", spec.alias.as_str(), "--json"]
    );
}

#[test]
fn interrupted_atomic_owner_publication_recovers_only_exact_known_links() {
    let directory = Directory::new();
    let spec = directory.spec();
    establish(&spec);
    let marker = spec.state_dir.join(".tv-control-owner");
    let pending = spec.state_dir.join(".tv-control-owner.pending");
    fs::hard_link(&marker, &pending).unwrap();
    let mock = Mock::new(vec![status(&spec, true)]);
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    assert!(OfficialTunnel { process: &mock }
        .status(&spec, &ctx)
        .unwrap()
        .locally_ready());
    assert!(!pending.exists());
    // A crash after fsync but before publication contains only the complete pending file.
    fs::rename(&marker, &pending).unwrap();
    fs::remove_dir(spec.state_dir.join("home")).unwrap();
    fs::remove_dir(spec.state_dir.join("profiles")).unwrap();
    establish(&spec);
    assert!(marker.exists());
    assert!(!pending.exists());
    // An incomplete/unrecognized candidate is preserved and never launches a runtime.
    fs::remove_file(&marker).unwrap();
    fs::remove_dir(spec.state_dir.join("home")).unwrap();
    fs::remove_dir(spec.state_dir.join("profiles")).unwrap();
    fs::write(&pending, b"partial").unwrap();
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o600)).unwrap();
    let mock = Mock::new(vec![]);
    assert_eq!(
        OfficialTunnel { process: &mock }
            .ensure(&spec, true, None, &ctx)
            .unwrap_err()
            .code,
        "tunnel_owner_recovery_required"
    );
    assert_eq!(fs::read(&pending).unwrap(), b"partial");
    assert!(mock.invocations.lock().unwrap().is_empty());
}

#[test]
fn malformed_identifiers_symlinks_and_cancellation_cannot_start_a_runtime() {
    let directory = Directory::new();
    let mut spec = directory.spec();
    spec.tunnel_id = "tunnel_abé_0123456789abcdef0123456789abcdef".into();
    assert!(spec.validate().is_err());
    spec.tunnel_id = "tunnel_ab12_0123456789abcdef0123456789abcdef".into();
    assert!(spec.validate().is_ok());
    spec.runtime_key_env = "OPENAI_ADMIN_KEY".into();
    assert!(spec.validate().is_err());
    let spec = directory.spec();
    std::os::unix::fs::symlink(&directory.0, &spec.state_dir).unwrap();
    let mock = Mock::new(vec![]);
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    assert_eq!(
        OfficialTunnel { process: &mock }
            .ensure(&spec, true, None, &context(&clock, &cancel))
            .unwrap_err()
            .code,
        "tunnel_directory_not_private"
    );
    fs::remove_file(&spec.state_dir).unwrap();
    cancel.cancel();
    assert_eq!(
        OfficialTunnel { process: &mock }
            .ensure(&spec, true, None, &context(&clock, &cancel))
            .unwrap_err()
            .code,
        "cancelled"
    );
    assert!(!spec.state_dir.exists());
    assert!(mock.invocations.lock().unwrap().is_empty());
}
