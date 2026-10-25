//! Command composition uses private files and an injected platform process only.
use super::*;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
use tv_adapters::{
    process::{Invocation, Output, Process},
    runtime::CancelFlag,
};
use tv_application::supervision::{self as policy, StopFence};
use tv_domain::Effect;

#[derive(Default)]
struct TestClock(AtomicU64);
impl Clock for TestClock {
    fn unix_ms(&self) -> u64 {
        self.monotonic_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}
fn context<'a>(clock: &'a TestClock, cancel: &'a CancelFlag) -> Context<'a> {
    Context {
        clock,
        cancellation: cancel,
        deadline_monotonic_ms: 1000,
    }
}

struct Fixture {
    root: PathBuf,
    binding: Binding,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "tv-service-command-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        for directory in ["connection", "prefix", "LaunchAgents"] {
            let path = root.join(directory);
            fs::create_dir(&path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let configuration = root.join("config.json");
        let launcher = root.join("prefix/tv-control");
        for (path, bytes) in [
            (configuration.as_path(), b"{}".as_slice()),
            (launcher.as_path(), b"fixture launcher".as_slice()),
            (
                root.join("identity").as_path(),
                b"preserve fixture identity".as_slice(),
            ),
            (
                root.join("journal").as_path(),
                b"preserve unknown fixture receipt".as_slice(),
            ),
        ] {
            fs::write(path, bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let binding = Binding {
            installation_id: "fixture_installation".into(),
            tunnel_id: "tunnel_fixture".into(),
            configuration,
            launcher,
            helper: root.join("tunnel-client"),
            executable_path: "/usr/bin:/bin".into(),
            label: "org.tv-control.command-fixture".into(),
            artifact: root.join("LaunchAgents/command-fixture.plist"),
            uid: 501,
            pins: BTreeMap::from([(
                root.join("identity").to_str().unwrap().into(),
                "a".repeat(64),
            )]),
        };
        Self { root, binding }
    }
    fn connection(&self) -> PathBuf {
        self.root.join("connection")
    }
    fn prepare(&self) -> SupervisionStore {
        SupervisionStore::prepare(&self.connection(), &self.binding, true).unwrap()
    }
    fn preserved(&self) {
        assert_eq!(fs::read(&self.binding.configuration).unwrap(), b"{}");
        assert_eq!(
            fs::read(self.root.join("identity")).unwrap(),
            b"preserve fixture identity"
        );
        assert_eq!(
            fs::read(self.root.join("journal")).unwrap(),
            b"preserve unknown fixture receipt"
        );
    }
    fn artifact(&self) {
        registration_marker::create(&self.binding).unwrap();
        artifact::create(
            &self.binding.artifact,
            &spec(&self.binding).render().unwrap(),
        )
        .unwrap();
    }
    fn installed_stopped(&self, store: &SupervisionStore) -> Record {
        let mut next = store.load().unwrap();
        next.revision += 1;
        next.arm = 1;
        next.registration = Registration::Installed;
        next.mode = Mode::Stopped;
        next.code = Code::CleanupVerified;
        assert!(store.compare_and_set(next.revision - 1, &next).unwrap());
        next
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

type Callback<'a> = dyn Fn(&Invocation, &Context<'_>) -> PortResult<Output> + Send + Sync + 'a;
struct MockProcess<'a> {
    callback: Box<Callback<'a>>,
    calls: Mutex<Vec<Invocation>>,
}
impl<'a> MockProcess<'a> {
    fn new(
        callback: impl Fn(&Invocation, &Context<'_>) -> PortResult<Output> + Send + Sync + 'a,
    ) -> Self {
        Self {
            callback: Box::new(callback),
            calls: Mutex::new(vec![]),
        }
    }
    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}
impl Process for MockProcess<'_> {
    fn run(&self, invocation: Invocation, ctx: &Context<'_>) -> PortResult<Output> {
        let result = (self.callback)(&invocation, ctx);
        self.calls.lock().unwrap().push(invocation);
        result
    }
}
fn acknowledged() -> PortResult<Output> {
    Ok(Output {
        success: true,
        stdout: b"IGNORED_FIXTURE_SECRET".to_vec(),
    })
}
fn platform_call(invocation: &Invocation, operation: &str) {
    assert_eq!(invocation.program, Path::new("/bin/launchctl"));
    assert_eq!(invocation.args[0], operation);
    assert!(invocation.mutating);
    assert!(invocation.stdin.is_empty());
    assert_eq!(invocation.env.len(), 1);
    assert_eq!(
        invocation.env.get(std::ffi::OsStr::new("PATH")).unwrap(),
        "/usr/bin:/bin"
    );
    assert!(!invocation.args.iter().any(|arg| arg == "-k"));
}

#[test]
fn bootstrap_observes_prepared_stopped_artifact_before_its_only_os_call() {
    let fixture = Fixture::new();
    let store = fixture.prepare();
    let process = MockProcess::new(|invocation, _| {
        platform_call(invocation, "bootstrap");
        assert_eq!(store.load().unwrap(), Record::prepared());
        registration_marker::verify(&fixture.binding).unwrap();
        artifact::verify(
            &fixture.binding.artifact,
            &spec(&fixture.binding).render().unwrap(),
        )
        .unwrap();
        assert_eq!(invocation.args.len(), 3);
        assert_eq!(invocation.args[1], "gui/501");
        assert_eq!(invocation.args[2], fixture.binding.artifact.as_os_str());
        acknowledged()
    });
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    assert_eq!(
        commands::bootstrap_prepared(&store, &process, &context(&clock, &cancel)).unwrap(),
        0
    );
    let record = store.load().unwrap();
    assert_eq!(
        (
            record.registration,
            record.mode,
            record.phase,
            record.arm,
            record.starts
        ),
        (Registration::Installed, Mode::Stopped, Phase::Idle, 0, 0)
    );
    assert_eq!(process.count(), 1);
    assert!(
        !String::from_utf8(fs::read(store.directory().join("state.json")).unwrap())
            .unwrap()
            .contains("IGNORED_FIXTURE_SECRET")
    );
    fixture.preserved();
}

#[test]
fn failed_or_lost_bootstrap_ack_is_unknown_disarmed_and_not_retried() {
    for lost in [false, true] {
        let fixture = Fixture::new();
        let store = fixture.prepare();
        let process = MockProcess::new(|invocation, _| {
            platform_call(invocation, "bootstrap");
            if lost {
                Err(Failure::uncertain("fixture_ack_lost"))
            } else {
                Ok(Output {
                    success: false,
                    stdout: vec![],
                })
            }
        });
        let clock = TestClock::default();
        let cancel = CancelFlag::default();
        let ctx = context(&clock, &cancel);
        let error = commands::bootstrap_prepared(&store, &process, &ctx).unwrap_err();
        assert_eq!(error.effect, Effect::MayHaveChanged);
        let after = store.load().unwrap();
        assert_eq!(
            (after.registration, after.mode, after.phase),
            (Registration::Unknown, Mode::Stopped, Phase::Idle)
        );
        assert!(fixture.binding.artifact.is_file());
        assert!(commands::bootstrap_prepared(&store, &process, &ctx).is_err());
        assert_eq!(store.load().unwrap(), after);
        assert_eq!(process.count(), 1);
        fixture.preserved();
    }
}

#[test]
fn cancellation_after_bootstrap_dispatch_keeps_unknown_disarmed_receipt() {
    let fixture = Fixture::new();
    let store = fixture.prepare();
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let process = MockProcess::new(|invocation, _| {
        platform_call(invocation, "bootstrap");
        cancel.cancel();
        acknowledged()
    });
    let error =
        commands::bootstrap_prepared(&store, &process, &context(&clock, &cancel)).unwrap_err();
    assert_eq!(error.effect, Effect::MayHaveChanged);
    let after = store.load().unwrap();
    assert_eq!(
        (after.registration, after.mode),
        (Registration::Unknown, Mode::Stopped)
    );
    assert_eq!(process.count(), 1);
    fixture.preserved();
}

#[test]
fn bootstrap_never_overwrites_a_newer_disarmed_revision() {
    let fixture = Fixture::new();
    let store = fixture.prepare();
    let process = MockProcess::new(|invocation, ctx| {
        platform_call(invocation, "bootstrap");
        policy::request_stop(&store, ctx).unwrap();
        // Stopping an already Stopped record may be idempotent. Model a newer
        // durable control-plane publication explicitly, to exercise the CAS
        // fence rather than requiring an unnecessary revision bump on stop.
        let mut newer = store.load().unwrap();
        newer.revision += 1;
        newer.code = Code::Stopped;
        assert!(store.compare_and_set(newer.revision - 1, &newer).unwrap());
        acknowledged()
    });
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let error =
        commands::bootstrap_prepared(&store, &process, &context(&clock, &cancel)).unwrap_err();
    assert_eq!(error.effect, Effect::MayHaveChanged);
    let current = store.load().unwrap();
    assert_eq!(
        (current.registration, current.mode, current.code),
        (Registration::Prepared, Mode::Stopped, Code::Stopped)
    );
    assert_eq!(process.count(), 1);
    fixture.preserved();
}

#[test]
fn bootstrap_with_cancelled_context_creates_no_artifact_or_platform_action() {
    let fixture = Fixture::new();
    let store = fixture.prepare();
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    cancel.cancel();
    let process = MockProcess::new(|_, _| panic!("cancelled bootstrap dispatched"));
    assert_eq!(
        commands::bootstrap_prepared(&store, &process, &context(&clock, &cancel))
            .unwrap_err()
            .effect,
        Effect::None
    );
    assert!(!fixture.binding.artifact.exists());
    assert!(!registration_marker::path(&fixture.binding)
        .unwrap()
        .exists());
    assert_eq!(store.load().unwrap(), Record::prepared());
    assert_eq!(process.count(), 0);
}

#[test]
fn uninstall_requires_current_arm_and_unchanged_owned_files_before_bootout() {
    for damage in ["old_arm", "artifact", "marker", "missing_marker"] {
        let fixture = Fixture::new();
        let store = fixture.prepare();
        fixture.artifact();
        let current = fixture.installed_stopped(&store);
        let fence = StopFence {
            arm: if damage == "old_arm" {
                current.arm - 1
            } else {
                current.arm
            },
        };
        if damage == "artifact" {
            fs::write(&fixture.binding.artifact, b"foreign artifact").unwrap();
        } else if damage == "marker" {
            fs::write(
                registration_marker::path(&fixture.binding).unwrap(),
                b"foreign marker",
            )
            .unwrap();
        } else if damage == "missing_marker" {
            fs::remove_file(registration_marker::path(&fixture.binding).unwrap()).unwrap();
        }
        let before = fs::read(&fixture.binding.artifact).unwrap();
        let process = MockProcess::new(|_, _| panic!("unsafe removal dispatched"));
        let clock = TestClock::default();
        let cancel = CancelFlag::default();
        assert!(
            commands::remove_stopped(&store, &fence, &process, &context(&clock, &cancel)).is_err()
        );
        assert_eq!(process.count(), 0);
        assert_eq!(store.load().unwrap(), current);
        assert_eq!(fs::read(&fixture.binding.artifact).unwrap(), before);
        fixture.preserved();
    }
}

#[test]
fn uninstall_does_not_remove_artifact_or_state_after_failed_bootout() {
    let fixture = Fixture::new();
    let store = fixture.prepare();
    fixture.artifact();
    let current = fixture.installed_stopped(&store);
    let bytes = fs::read(&fixture.binding.artifact).unwrap();
    let process = MockProcess::new(|invocation, _| {
        platform_call(invocation, "bootout");
        Err(Failure::uncertain("fixture_unload_unknown"))
    });
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let error = commands::remove_stopped(
        &store,
        &StopFence { arm: current.arm },
        &process,
        &context(&clock, &cancel),
    )
    .unwrap_err();
    assert_eq!(error.effect, Effect::MayHaveChanged);
    assert_eq!(store.load().unwrap(), current);
    assert_eq!(fs::read(&fixture.binding.artifact).unwrap(), bytes);
    assert_eq!(process.count(), 1);
    fixture.preserved();
}

#[test]
fn uninstall_keeps_a_new_arm_and_its_artifact_after_late_bootout_ack() {
    let fixture = Fixture::new();
    let store = fixture.prepare();
    fixture.artifact();
    let current = fixture.installed_stopped(&store);
    let process = MockProcess::new(|invocation, _| {
        platform_call(invocation, "bootout");
        let mut next = store.load().unwrap();
        next.revision += 1;
        next.arm += 1;
        assert!(store.compare_and_set(next.revision - 1, &next).unwrap());
        acknowledged()
    });
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let error = commands::remove_stopped(
        &store,
        &StopFence { arm: current.arm },
        &process,
        &context(&clock, &cancel),
    )
    .unwrap_err();
    assert_eq!(error.effect, Effect::MayHaveChanged);
    assert_eq!(store.load().unwrap().arm, current.arm + 1);
    assert!(fixture.binding.artifact.exists());
    assert_eq!(process.count(), 1);
    fixture.preserved();
}

#[test]
fn acknowledged_uninstall_removes_only_its_owned_service_files() {
    let fixture = Fixture::new();
    let store = fixture.prepare();
    fixture.artifact();
    let current = fixture.installed_stopped(&store);
    let process = MockProcess::new(|invocation, _| {
        platform_call(invocation, "bootout");
        assert_eq!(store.load().unwrap(), current);
        assert!(fixture.binding.artifact.exists());
        acknowledged()
    });
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    assert_eq!(
        commands::remove_stopped(
            &store,
            &StopFence { arm: current.arm },
            &process,
            &context(&clock, &cancel)
        )
        .unwrap(),
        0
    );
    assert!(!fixture.binding.artifact.exists());
    assert!(!registration_marker::path(&fixture.binding)
        .unwrap()
        .exists());
    assert!(!fixture.connection().join(DIRECTORY).exists());
    assert_eq!(process.count(), 1);
    assert!(removal_check(&fixture.binding.configuration, &fixture.binding.launcher).is_ok());
    fixture.preserved();
}

#[test]
fn installer_guard_refuses_live_unknown_unsafe_or_unremoved_state_without_changes() {
    for registration in [
        Registration::Prepared,
        Registration::Unknown,
        Registration::Installed,
        Registration::Removed,
    ] {
        let fixture = Fixture::new();
        let store = fixture.prepare();
        let mut record = store.load().unwrap();
        record.revision += 1;
        record.registration = registration;
        assert!(store.compare_and_set(record.revision - 1, &record).unwrap());
        fixture.artifact();
        let state = fs::read(store.directory().join("state.json")).unwrap();
        assert!(removal_check(&fixture.binding.configuration, &fixture.binding.launcher).is_err());
        assert_eq!(
            fs::read(store.directory().join("state.json")).unwrap(),
            state
        );
        assert!(fixture.binding.artifact.exists());
        fixture.preserved();
    }
    let fixture = Fixture::new();
    let store = fixture.prepare();
    fs::write(store.directory().join("state.json"), b"corrupt").unwrap();
    assert!(removal_check(&fixture.binding.configuration, &fixture.binding.launcher).is_err());
    assert_eq!(
        fs::read(store.directory().join("state.json")).unwrap(),
        b"corrupt"
    );
}

#[test]
fn installer_guard_requires_absent_artifact_and_released_owner_for_removed_state() {
    let fixture = Fixture::new();
    let store = fixture.prepare();
    let mut record = store.load().unwrap();
    record.revision += 1;
    record.registration = Registration::Removed;
    assert!(store.compare_and_set(record.revision - 1, &record).unwrap());
    let owner = store.try_owner().unwrap();
    assert!(removal_check(&fixture.binding.configuration, &fixture.binding.launcher).is_err());
    drop(owner);
    assert!(removal_check(&fixture.binding.configuration, &fixture.binding.launcher).is_ok());
    fixture.preserved();
}

#[test]
fn missing_approval_precedes_configuration_or_service_effects() {
    let fixture = Fixture::new();
    let missing = fixture
        .root
        .join("missing.json")
        .to_string_lossy()
        .into_owned();
    for action in ["install", "resume"] {
        assert_eq!(
            run(&[action.into(), missing.clone()]),
            Err("service_persistence_approval_required")
        );
    }
    assert!(!fixture.connection().join(DIRECTORY).exists());
    assert!(!fixture.binding.artifact.exists());
    fixture.preserved();
}

#[test]
fn configuration_guard_rejects_links_directories_and_public_files() {
    let fixture = Fixture::new();
    assert_eq!(
        connection(&fixture.binding.configuration).unwrap(),
        fixture.connection()
    );
    // Relative CLI paths resolve through the current directory, without changing
    // process-global cwd while other fixture tests may be running.
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    let cwd_parts: Vec<_> = cwd.components().collect();
    let target_parts: Vec<_> = fixture.binding.configuration.components().collect();
    let common = cwd_parts
        .iter()
        .zip(&target_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..cwd_parts.len() {
        relative.push("..");
    }
    for part in &target_parts[common..] {
        relative.push(part.as_os_str());
    }
    assert!(!relative.is_absolute());
    assert_eq!(connection(&relative).unwrap(), fixture.connection());
    let alias = fixture.root.join("config-link");
    symlink(&fixture.binding.configuration, &alias).unwrap();
    assert_eq!(
        connection(&alias).unwrap_err().code,
        "supervision_configuration_unsafe"
    );
    fs::remove_file(alias).unwrap();
    let hardlink = fixture.root.join("config-hardlink");
    fs::hard_link(&fixture.binding.configuration, &hardlink).unwrap();
    assert_eq!(
        connection(&fixture.binding.configuration).unwrap_err().code,
        "supervision_configuration_unsafe"
    );
    fs::remove_file(hardlink).unwrap();
    fs::set_permissions(
        &fixture.binding.configuration,
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert_eq!(
        connection(&fixture.binding.configuration).unwrap_err().code,
        "supervision_configuration_unsafe"
    );
    fs::set_permissions(
        &fixture.binding.configuration,
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(
        connection(&fixture.connection()).unwrap_err().code,
        "supervision_configuration_unsafe"
    );
    let alias = fixture.root.join("directory-link");
    symlink(&fixture.root, &alias).unwrap();
    assert_eq!(
        connection(&alias.join("config.json")).unwrap_err().code,
        "supervision_configuration_unsafe"
    );
    assert!(!fixture.connection().join(DIRECTORY).exists());
    fixture.preserved();
}

#[test]
fn artifact_rejects_existing_parent_symlink_before_creating_any_child() {
    let fixture = Fixture::new();
    let target = fixture.root.join("foreign-target");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(target.join("sentinel"), b"preserve target").unwrap();
    let alias = fixture.root.join("parent-alias");
    symlink(&target, &alias).unwrap();
    let artifact = alias.join("new-child/service.plist");
    assert!(artifact::create(&artifact, b"fixture artifact").is_err());
    assert!(!target.join("new-child").exists());
    assert_eq!(
        fs::read(target.join("sentinel")).unwrap(),
        b"preserve target"
    );
    assert!(alias.symlink_metadata().unwrap().file_type().is_symlink());
    fixture.preserved();
}

#[test]
fn installation_lock_excludes_concurrent_or_foreign_locks_and_releases_exactly() {
    let fixture = Fixture::new();
    let path = fixture.root.join(".prefix.install-lock");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("foreign"), b"preserve").unwrap();
    assert!(installation_lock::InstallationLock::acquire(&fixture.binding.launcher).is_err());
    assert_eq!(fs::read(path.join("foreign")).unwrap(), b"preserve");
    fs::remove_file(path.join("foreign")).unwrap();
    fs::remove_dir(&path).unwrap();
    let owner = installation_lock::InstallationLock::acquire(&fixture.binding.launcher).unwrap();
    assert!(installation_lock::InstallationLock::acquire(&fixture.binding.launcher).is_err());
    owner.release().unwrap();
    assert!(!path.exists());
    installation_lock::InstallationLock::acquire(&fixture.binding.launcher)
        .unwrap()
        .release()
        .unwrap();
    assert!(!path.exists());
}

#[test]
fn installation_lock_preserves_replacements_and_never_recursively_removes_contents() {
    for replacement in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.root.join(".prefix.install-lock");
        let owner =
            installation_lock::InstallationLock::acquire(&fixture.binding.launcher).unwrap();
        if replacement {
            fs::rename(&path, fixture.root.join("old-owned-lock")).unwrap();
            fs::create_dir(&path).unwrap();
        }
        fs::write(path.join("foreign"), b"preserve").unwrap();
        assert_eq!(
            owner.release().unwrap_err().code,
            "installation_lock_cleanup_unverified"
        );
        assert_eq!(fs::read(path.join("foreign")).unwrap(), b"preserve");
        assert!(path.is_dir());
        fixture.preserved();
    }
}
