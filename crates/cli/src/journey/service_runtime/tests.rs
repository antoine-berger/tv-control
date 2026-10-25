use super::*;
use serde_json::{json, Value};
use std::{
    fs,
    net::TcpListener,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    sync::atomic::{AtomicU64, AtomicUsize},
};
use tv_adapters::{
    onboarding_tunnel::TunnelSpec,
    pipeline_store::{Checkpoint, ProgressOutcome, ProgressStage},
    process::{BoundedProcess, Invocation, Output},
    runtime::{CancelFlag, SystemClock},
};
use tv_application::generic::Journal as _;
use tv_application::supervision::Store;
use tv_application::{supervision, Clock};

struct Fixture {
    root: PathBuf,
    configuration: PathBuf,
    artifact: PathBuf,
    state: PathBuf,
    process: Fake,
}
struct Fake {
    spec: TunnelSpec,
    running: AtomicBool,
    connects: AtomicUsize,
    stops: AtomicUsize,
    status_reads: AtomicUsize,
    fail_start: AtomicBool,
    old_protocol: AtomicBool,
}
impl Fake {
    fn status(&self) -> Value {
        let running = self.running.load(Ordering::SeqCst);
        json!({"alias":self.spec.alias,"tunnel_id":self.spec.tunnel_id,
            "profile_name":self.spec.alias,"profile_dir":self.spec.state_dir.join("profiles"),
            "stale":false,"process_running":running,"healthy":running,"ready":running,
            "process":{"alias":self.spec.alias,"tunnel_id":self.spec.tunnel_id,
                "profile_name":self.spec.alias,"profile_dir":self.spec.state_dir.join("profiles"),
                "target_kind":"command","target_value":self.spec.mcp_command().unwrap()}})
    }
}
impl Process for Fake {
    fn run(&self, invocation: Invocation, ctx: &Context<'_>) -> PortResult<Output> {
        ctx.check()?;
        if invocation
            .program
            .file_name()
            .is_some_and(|n| n == "shasum" || n == "sha256sum")
        {
            // Exercise the production digest wrapper against real private fixture
            // files with the existing bounded system tool, not a new hash implementation.
            assert!(!invocation.mutating);
            return BoundedProcess {
                max_output: 65_536,
                poll_ms: 1,
            }
            .run(invocation, ctx);
        }
        if invocation.program == self.spec.mcp_binary {
            assert_eq!(
                invocation.args,
                ["service", "protocol"].map(std::ffi::OsString::from)
            );
            assert!(!invocation.mutating);
            return Ok(Output {
                success: true,
                stdout: if self.old_protocol.load(Ordering::SeqCst) {
                    b"old CLI\n".to_vec()
                } else {
                    b"TV_CONTROL_SUPERVISION_V1\n".to_vec()
                },
            });
        }
        assert_eq!(invocation.program, self.spec.binary);
        let args: Vec<_> = invocation
            .args
            .iter()
            .map(|a| a.to_str().unwrap())
            .collect();
        let value = match args.get(0..2) {
            Some(["runtimes", "status"]) => {
                assert!(!invocation.mutating);
                self.status_reads.fetch_add(1, Ordering::SeqCst);
                self.status()
            }
            Some(["runtimes", "list"]) => {
                assert!(!invocation.mutating);
                json!({"aliases":[{"alias":self.spec.alias,"tunnel_id":self.spec.tunnel_id,
                    "profile_name":self.spec.alias,"profile_dir":self.spec.state_dir.join("profiles")}]})
            }
            Some(["runtimes", "connect"]) => {
                assert!(invocation.mutating);
                assert_eq!(
                    invocation
                        .env
                        .get(std::ffi::OsStr::new("TV_CONTROL_TUNNEL_KEY"))
                        .unwrap(),
                    "fixture-saved-key"
                );
                self.connects.fetch_add(1, Ordering::SeqCst);
                if self.fail_start.load(Ordering::SeqCst) {
                    return Err(Failure::uncertain("fixture_start_ack_lost"));
                }
                runtime_intent::start_generation(
                    self.spec.bridge_state_dir.as_ref().unwrap(),
                    "installation",
                    "revision",
                    "screen",
                )
                .unwrap();
                self.running.store(true, Ordering::SeqCst);
                json!({"launched":true})
            }
            Some(["runtimes", "stop"]) => {
                assert!(invocation.mutating);
                self.stops.fetch_add(1, Ordering::SeqCst);
                self.running.store(false, Ordering::SeqCst);
                self.status()
            }
            _ => panic!("unexpected fixture process"),
        };
        Ok(Output {
            success: true,
            stdout: serde_json::to_vec(&value).unwrap(),
        })
    }
}
fn directory(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn file(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // Production also pins current_exe. Anchor fixture executables under its
        // already trusted ancestry rather than Linux's intentionally public /tmp.
        let binary = std::env::current_exe().unwrap().canonicalize().unwrap();
        let root = binary.parent().unwrap().join(format!(
            "tv-service-binding-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        directory(&root);
        assert_eq!(root.canonicalize().unwrap(), root);
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        let state = root.join("connection");
        directory(&state);
        for path in [
            "adb",
            "adb/adb-home",
            "adb/adb-home/.android",
            "tunnel",
            "tunnel/profiles",
            "tunnel/home",
        ] {
            directory(&state.join(path));
        }
        file(
            &state.join("adb/adb-home/.android/adbkey"),
            b"fixture-existing-private-key",
            0o600,
        );
        file(
            &state.join("adb/adb-home/.android/adbkey.pub"),
            b"fixture-existing-public-key",
            0o644,
        );
        file(&state.join("session.lock"), b"", 0o600);
        file(&state.join("events.jsonl"), b"", 0o600);
        let configuration = root.join("tv.json");
        let launcher = root.join("launcher");
        let helper = root.join("tunnel-client");
        let adb = root.join("adb");
        let curl = root.join("curl");
        for path in [&launcher, &helper, &adb, &curl] {
            file(path, b"#!/bin/sh\nexit 0\n", 0o700);
        }
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut config: Value =
            serde_json::from_slice(include_bytes!("../../../../../examples/config.json")).unwrap();
        config["generic_adb_control"] = json!(true);
        config["executables"] = json!({"adb":adb,"curl":curl});
        config["devices"] = json!({"screen":{"adapter":"adb","coordination_group":"fixture-tv","endpoint":"192.0.2.10:5555", "server_host":"127.0.0.1","server_port":port,"media_app":null,"volume_max_index":null}});
        for field in ["apps", "servers", "media", "workflows"] {
            config[field] = json!({});
        }
        let snapshot = serde_json::to_vec(&config).unwrap();
        file(&configuration, &snapshot, 0o600);
        let checkpoint = Checkpoint {
            schema: 1,
            installation_id: "installation".into(),
            configuration_path: configuration.clone(),
            configuration_revision: "revision".into(),
            device: "screen".into(),
            stage: ProgressStage::Complete,
            outcome: ProgressOutcome::Verified,
            tunnel_id: Some("tunnel_0123456789abcdef0123456789abcdef".into()),
            tunnel_alias: Some("tv-installation".into()),
        };
        file(
            &state.join("progress.json"),
            &serde_json::to_vec(&checkpoint).unwrap(),
            0o600,
        );
        let intent = super::super::runtime::RuntimeIntent {
            schema: 1,
            installation_id: "installation".into(),
            revision: "revision".into(),
            device: "screen".into(),
            configuration: String::from_utf8(snapshot).unwrap(),
            controller_binary: launcher.clone(),
        };
        file(
            &state.join("runtime-intent.json"),
            &serde_json::to_vec(&intent).unwrap(),
            0o600,
        );
        runtime_credential::save(
            &state,
            "installation",
            checkpoint.tunnel_id.as_ref().unwrap(),
            "fixture-saved-key",
            true,
        )
        .unwrap();
        runtime_intent::start_generation(&state, "installation", "revision", "screen").unwrap();
        let spec = TunnelSpec {
            binary: helper,
            alias: "tv-installation".into(),
            tunnel_id: checkpoint.tunnel_id.unwrap(),
            runtime_key_env: "TV_CONTROL_TUNNEL_KEY".into(),
            state_dir: state.join("tunnel"),
            mcp_binary: launcher,
            configuration: configuration.clone(),
            bridge_state_dir: Some(state.clone()),
            executable_path: std::env::var_os("PATH").unwrap(),
        };
        file(&state.join("tunnel/.tv-control-owner"), &serde_json::to_vec(&json!({"schema":1,"alias":spec.alias,"tunnel_id":spec.tunnel_id,
            "mcp_command":spec.mcp_command().unwrap(),"profile_dir":spec.state_dir.join("profiles")})).unwrap(), 0o600);
        Self {
            artifact: root.join("service.plist"),
            root,
            configuration,
            state,
            process: Fake {
                spec,
                running: AtomicBool::new(false),
                connects: AtomicUsize::new(0),
                stops: AtomicUsize::new(0),
                status_reads: AtomicUsize::new(0),
                fail_start: AtomicBool::new(false),
                old_protocol: AtomicBool::new(false),
            },
        }
    }
    fn capture(&self, ctx: &Context<'_>) -> Binding {
        capture_existing(
            &self.configuration,
            &self.artifact,
            &self.process.spec.binary,
            unsafe { libc::geteuid() },
            &self.process,
            ctx,
        )
        .unwrap()
    }
    fn store(&self, binding: &Binding) -> SupervisionStore {
        let store = SupervisionStore::prepare(&self.state, binding, true).unwrap();
        let mut installed = store.load().unwrap();
        installed.revision += 1;
        installed.registration = Registration::Installed;
        assert!(store.compare_and_set(0, &installed).unwrap());
        let clock = SystemClock::default();
        let cancel = CancelFlag::default();
        let ctx = context(&clock, &cancel);
        supervision::rearm(&store, &ctx).unwrap();
        supervision::enter(&store, &ctx).unwrap();
        store
    }
    fn admit(store: &SupervisionStore) {
        let current = store.load().unwrap();
        let mut next = current.clone();
        next.revision += 1;
        next.starts += 1;
        next.phase = Phase::StartInFlight;
        next.code = Code::Starting;
        assert!(store.compare_and_set(current.revision, &next).unwrap());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn context<'a>(clock: &'a SystemClock, cancel: &'a CancelFlag) -> Context<'a> {
    Context {
        clock,
        cancellation: cancel,
        deadline_monotonic_ms: clock.monotonic_ms() + 20_000,
    }
}

fn preserved_unknown_journal(fixture: &Fixture) -> (PathBuf, Vec<u8>) {
    use tv_domain::generic::{Intent, Operation, Reply, Request};
    let path = fixture.root.join(".tv-control-operations");
    let request = Request {
        id: "preserved-layout".into(),
        device: "screen".into(),
        intent: Intent::Pause,
        expires_at_unix_ms: 1000,
    };
    {
        let journal =
            tv_adapters::operation_journal::OperationJournal::open(&path, 16, 60_000).unwrap();
        assert!(matches!(
            journal.reserve(&request, "fixture-tv", 100).unwrap(),
            tv_application::generic::Reservation::Acquired
        ));
        // This fixture tests preservation of an already published Unknown, not
        // crash recovery while another test's pre-exec child may inherit a lease.
        journal
            .finish(
                &request,
                &Reply {
                    request_id: request.id.clone(),
                    status: tv_domain::Status::OutcomeUnknown,
                    code: "fixture_unknown".into(),
                    observed: None,
                    choices: vec![],
                    cached: false,
                    local_elapsed_ms: 0,
                },
                200,
            )
            .unwrap();
    }
    let journal =
        tv_adapters::operation_journal::OperationJournal::open(&path, 16, 60_000).unwrap();
    assert!(
        matches!(journal.get(&request.id).unwrap(), Operation::Complete(reply) if reply.status == tv_domain::Status::OutcomeUnknown && reply.code == "fixture_unknown")
    );
    let file = path.join("operations.json");
    let bytes = fs::read(&file).unwrap();
    (file, bytes)
}

#[test]
fn existing_executable_symlink_is_bound_and_retargeting_blocks_without_lifecycle() {
    let fixture = Fixture::new();
    let adb = fixture.root.join("adb");
    let first = fixture.root.join("adb-v1");
    fs::rename(&adb, &first).unwrap();
    symlink(&first, &adb).unwrap();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    let binding = fixture.capture(&ctx);
    check_binding(&binding, &fixture.process, &ctx).unwrap();
    assert!(binding.pins.contains_key(adb.to_str().unwrap()));
    assert!(binding.pins.contains_key(first.to_str().unwrap()));
    let store = fixture.store(&binding);
    let backend =
        RuntimeBackend::new_with_context(&store, store.load().unwrap().arm, &fixture.process, &ctx)
            .unwrap();
    let replacement = fixture.root.join("adb-v2");
    file(&replacement, b"#!/bin/sh\nexit 1\n", 0o700);
    fs::remove_file(&adb).unwrap();
    symlink(&replacement, &adb).unwrap();
    assert_eq!(
        backend.observe_owned(&ctx).unwrap_err().code,
        "supervision_binding_changed"
    );
    assert_eq!(fixture.process.connects.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.process.status_reads.load(Ordering::SeqCst), 0);
}

#[test]
fn missing_or_symlink_identity_and_old_launcher_never_start() {
    let fixture = Fixture::new();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    fixture.process.old_protocol.store(true, Ordering::SeqCst);
    assert_eq!(
        capture_existing(
            &fixture.configuration,
            &fixture.artifact,
            &fixture.process.spec.binary,
            unsafe { libc::geteuid() },
            &fixture.process,
            &ctx
        )
        .err()
        .unwrap()
        .code,
        "supervision_launcher_protocol_unsupported"
    );
    fixture.process.old_protocol.store(false, Ordering::SeqCst);
    let binding = fixture.capture(&ctx);
    let key = fixture.state.join("adb/adb-home/.android/adbkey");
    let moved = fixture.state.join("adb/adb-home/.android/old-key");
    fs::rename(&key, &moved).unwrap();
    assert!(check_binding(&binding, &fixture.process, &ctx).is_err());
    symlink(&moved, &key).unwrap();
    assert!(check_binding(&binding, &fixture.process, &ctx).is_err());
    assert_eq!(fixture.process.connects.load(Ordering::SeqCst), 0);
    let root = fixture.root.clone();
    drop(fixture);
    assert!(
        !root.exists(),
        "private fixture root must be removed by Drop"
    );
}

#[test]
fn owned_absent_runtime_starts_once_with_new_generation_and_preserves_saved_access() {
    let fixture = Fixture::new();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    let binding = fixture.capture(&ctx);
    let android = fixture.state.join("adb/adb-home/.android");
    let keys: Vec<_> = [android.join("adbkey"), android.join("adbkey.pub")]
        .into_iter()
        .map(|path| {
            assert!(binding.pins.contains_key(path.to_str().unwrap()));
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    assert!(!fixture.state.join("adb/adb-home/adbkey").exists());
    let (journal, journal_bytes) = preserved_unknown_journal(&fixture);
    let store = fixture.store(&binding);
    let previous =
        runtime_intent::read_generation(&fixture.state, "installation", "revision", "screen")
            .unwrap();
    let backend =
        RuntimeBackend::new_with_context(&store, store.load().unwrap().arm, &fixture.process, &ctx)
            .unwrap();
    assert_eq!(backend.observe_owned(&ctx).unwrap(), Observation::Absent);
    Fixture::admit(&store);
    assert_eq!(backend.start_owned(&ctx).unwrap(), Observation::Ready);
    assert_ne!(
        runtime_intent::read_generation(&fixture.state, "installation", "revision", "screen")
            .unwrap(),
        previous
    );
    assert_eq!(fixture.process.connects.load(Ordering::SeqCst), 1);
    check_binding(&binding, &fixture.process, &ctx).unwrap();
    assert_eq!(backend.observe_owned(&ctx).unwrap(), Observation::Ready);
    assert_eq!(backend.stop_owned(&ctx).unwrap(), Observation::Absent);
    assert_eq!(fixture.process.stops.load(Ordering::SeqCst), 1);
    check_binding(&binding, &fixture.process, &ctx).unwrap();
    for (path, bytes) in keys {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert_eq!(fs::read(journal).unwrap(), journal_bytes);
}

#[test]
fn missing_or_symlink_android_identity_is_refused_without_fallback_or_repair() {
    for mutation in [
        "directory",
        "directory_symlink",
        "private",
        "private_symlink",
        "public",
        "public_symlink",
    ] {
        let fixture = Fixture::new();
        let clock = SystemClock::default();
        let cancel = CancelFlag::default();
        let ctx = context(&clock, &cancel);
        let binding = fixture.capture(&ctx);
        let store = fixture.store(&binding);
        let backend = RuntimeBackend::new_with_context(
            &store,
            store.load().unwrap().arm,
            &fixture.process,
            &ctx,
        )
        .unwrap();
        let (journal, journal_bytes) = preserved_unknown_journal(&fixture);
        let stable: Vec<_> = [
            &fixture.configuration,
            &fixture.state.join("runtime-credential.json"),
        ]
        .into_iter()
        .map(|path| (path.to_owned(), fs::read(path).unwrap()))
        .collect();
        let android = fixture.state.join("adb/adb-home/.android");
        let mut private = android.join("adbkey");
        let mut public = android.join("adbkey.pub");
        let private_bytes = fs::read(&private).unwrap();
        let public_bytes = fs::read(&public).unwrap();
        if mutation.starts_with("directory") {
            let saved = fixture.state.join("adb/adb-home/saved-android");
            fs::rename(&android, &saved).unwrap();
            private = saved.join("adbkey");
            public = saved.join("adbkey.pub");
            if mutation == "directory_symlink" {
                symlink(&saved, &android).unwrap();
            }
            // A complete flat pair must not become an implicit fallback.
            file(
                &fixture.state.join("adb/adb-home/adbkey"),
                b"fixture-flat-private",
                0o600,
            );
            file(
                &fixture.state.join("adb/adb-home/adbkey.pub"),
                b"fixture-flat-public",
                0o644,
            );
        } else {
            let path = if mutation.starts_with("private") {
                &mut private
            } else {
                &mut public
            };
            let original = path.clone();
            let saved = android.join("saved-key");
            fs::rename(&original, &saved).unwrap();
            if mutation.ends_with("symlink") {
                symlink(&saved, &original).unwrap();
            }
            *path = saved;
        }
        assert!(backend.observe_owned(&ctx).is_err(), "{mutation}");
        Fixture::admit(&store);
        assert!(backend.start_owned(&ctx).is_err(), "{mutation}");
        assert!(backend.stop_owned(&ctx).is_err(), "{mutation}");
        assert_eq!(
            fixture.process.status_reads.load(Ordering::SeqCst),
            0,
            "{mutation}"
        );
        assert_eq!(
            fixture.process.connects.load(Ordering::SeqCst),
            0,
            "{mutation}"
        );
        assert_eq!(
            fixture.process.stops.load(Ordering::SeqCst),
            0,
            "{mutation}"
        );
        assert_eq!(fs::read(private).unwrap(), private_bytes);
        assert_eq!(fs::read(public).unwrap(), public_bytes);
        assert_eq!(fs::read(journal).unwrap(), journal_bytes);
        for (path, bytes) in stable {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        if mutation == "directory" {
            assert!(!android.exists());
        }
    }
}

#[test]
fn occupied_adb_port_stopped_intent_and_uncertain_start_do_not_trigger_recovery() {
    let fixture = Fixture::new();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    let binding = fixture.capture(&ctx);
    let store = fixture.store(&binding);
    let arm = store.load().unwrap().arm;
    let backend = RuntimeBackend::new_with_context(&store, arm, &fixture.process, &ctx).unwrap();
    let config =
        tv_adapters::config::Config::from_json(&fs::read(&fixture.configuration).unwrap()).unwrap();
    let spec = super::super::runtime::adb_spec(&config, "screen", &fixture.state).unwrap();
    let occupied = TcpListener::bind(("127.0.0.1", spec.port)).unwrap();
    assert_eq!(
        backend.observe_owned(&ctx).unwrap_err().code,
        "supervision_unverified_adb_orphan"
    );
    drop(occupied);
    Fixture::admit(&store);
    fixture.process.fail_start.store(true, Ordering::SeqCst);
    assert_eq!(
        backend.start_owned(&ctx).unwrap_err().effect,
        Effect::MayHaveChanged
    );
    assert_eq!(
        backend.start_owned(&ctx).unwrap_err().code,
        "supervision_start_unresolved"
    );
    assert_eq!(
        backend.stop_owned(&ctx).unwrap_err().code,
        "supervision_cleanup_unverified"
    );
    let restarted = RuntimeBackend::new_with_context(&store, arm, &fixture.process, &ctx).unwrap();
    assert_eq!(
        restarted.stop_owned(&ctx).unwrap_err().code,
        "supervision_cleanup_unverified"
    );
    assert_eq!(fixture.process.connects.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.process.stops.load(Ordering::SeqCst), 0);
}

#[test]
fn explicit_stop_before_start_admission_prevents_dispatch() {
    let fixture = Fixture::new();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    let binding = fixture.capture(&ctx);
    let store = fixture.store(&binding);
    let backend =
        RuntimeBackend::new_with_context(&store, store.load().unwrap().arm, &fixture.process, &ctx)
            .unwrap();
    Fixture::admit(&store);
    supervision::request_stop(&store, &ctx).unwrap();
    assert_eq!(
        backend.start_owned(&ctx).unwrap_err().code,
        "supervision_stopped"
    );
    assert_eq!(fixture.process.connects.load(Ordering::SeqCst), 0);
}

#[test]
fn normal_journal_changes_are_allowed_but_saved_key_rotation_blocks_before_status() {
    let fixture = Fixture::new();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    let binding = fixture.capture(&ctx);
    let store = fixture.store(&binding);
    let backend =
        RuntimeBackend::new_with_context(&store, store.load().unwrap().arm, &fixture.process, &ctx)
            .unwrap();
    let journal = fixture.root.join(".tv-control-operations");
    directory(&journal);
    file(
        &journal.join("operations.json"),
        b"fixture mutable journal bytes",
        0o600,
    );
    check_binding(&binding, &fixture.process, &ctx).unwrap();
    assert!(!binding
        .pins
        .contains_key(journal.join("operations.json").to_str().unwrap()));
    runtime_credential::save(
        &fixture.state,
        "installation",
        &binding.tunnel_id,
        "fixture-rotated-key",
        true,
    )
    .unwrap();
    assert_eq!(
        backend.observe_owned(&ctx).unwrap_err().code,
        "supervision_binding_changed"
    );
    assert_eq!(fixture.process.status_reads.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.process.connects.load(Ordering::SeqCst), 0);
    assert_eq!(
        fs::read(journal.join("operations.json")).unwrap(),
        b"fixture mutable journal bytes"
    );
}

#[test]
fn missing_existing_profile_and_cancellation_never_create_or_start() {
    let fixture = Fixture::new();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    let binding = fixture.capture(&ctx);
    let store = fixture.store(&binding);
    let backend =
        RuntimeBackend::new_with_context(&store, store.load().unwrap().arm, &fixture.process, &ctx)
            .unwrap();
    Fixture::admit(&store);
    let profiles = fixture.state.join("tunnel/profiles");
    fs::remove_dir(&profiles).unwrap();
    assert!(backend.start_owned(&ctx).is_err());
    assert!(!profiles.exists());
    assert_eq!(fixture.process.connects.load(Ordering::SeqCst), 0);
    cancel.cancel();
    assert_eq!(
        backend.observe_owned(&ctx).unwrap_err().status(),
        tv_domain::Status::Cancelled
    );
    assert_eq!(fixture.process.status_reads.load(Ordering::SeqCst), 0);
}

fn transaction_lock(store: &SupervisionStore) -> fs::File {
    use std::os::fd::AsRawFd;
    let file = fs::File::open(store.directory().join("transaction.lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    file
}
struct UnlockOnWait {
    clock: SystemClock,
    lock: Mutex<Option<fs::File>>,
}
impl Clock for UnlockOnWait {
    fn unix_ms(&self) -> u64 {
        self.clock.unix_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.clock.monotonic_ms()
    }
    fn sleep_ms(&self, ms: u64) {
        use std::os::fd::AsRawFd;
        if let Some(lock) = self.lock.lock().unwrap().take() {
            assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) }, 0);
        }
        self.clock.sleep_ms(ms);
    }
}

#[test]
fn short_transaction_contention_neither_latches_unknown_nor_cancels_the_fence() {
    let fixture = Fixture::new();
    let original_clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let initial = context(&original_clock, &cancel);
    let binding = fixture.capture(&initial);
    let store = fixture.store(&binding);
    let arm = store.load().unwrap().arm;
    let before = store.load().unwrap();
    let clock = UnlockOnWait {
        clock: SystemClock::default(),
        lock: Mutex::new(Some(transaction_lock(&store))),
    };
    let ctx = Context {
        clock: &clock,
        cancellation: &cancel,
        deadline_monotonic_ms: 20_000,
    };
    // Releasing only at the first bounded lock-wait makes contention deterministic.
    let backend = RuntimeBackend::new_with_context(&store, arm, &fixture.process, &ctx).unwrap();
    assert!(!backend.unresolved.load(Ordering::Acquire));
    assert!(clock.lock.lock().unwrap().is_none());
    *clock.lock.lock().unwrap() = Some(transaction_lock(&store));
    let fence = Fence {
        store: &store,
        arm,
        outer: Context {
            clock: &clock,
            cancellation: &cancel,
            deadline_monotonic_ms: ctx.deadline_monotonic_ms,
        },
        start: true,
        error: Mutex::new(None),
    };
    let fenced = Context {
        clock: &clock,
        cancellation: &fence,
        deadline_monotonic_ms: ctx.deadline_monotonic_ms,
    };
    fenced.check().unwrap();
    assert!(fence.error.lock().unwrap().is_none());
    assert!(clock.lock.lock().unwrap().is_none());
    assert_eq!(store.load().unwrap(), before);
    assert_eq!(fixture.process.connects.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.process.stops.load(Ordering::SeqCst), 0);
}

#[test]
fn persistent_contention_returns_an_error_without_fabricating_unresolved_start() {
    let fixture = Fixture::new();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    let binding = fixture.capture(&ctx);
    let store = fixture.store(&binding);
    let arm = store.load().unwrap().arm;
    let lock = transaction_lock(&store);
    let started = std::time::Instant::now();
    let error = RuntimeBackend::new_with_context(&store, arm, &fixture.process, &ctx)
        .err()
        .unwrap();
    assert_eq!(error.code, "supervision_store_busy");
    assert_eq!(error.effect, Effect::None);
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(lock);
    let backend = RuntimeBackend::new_with_context(&store, arm, &fixture.process, &ctx).unwrap();
    assert!(!backend.unresolved.load(Ordering::Acquire));
    assert_eq!(fixture.process.connects.load(Ordering::SeqCst), 0);
}
