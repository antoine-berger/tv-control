//! Only an injected process is used for lifecycle commands. Never loads a job.
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
use tv_adapters::{
    process::{Invocation, Output, Process},
    runtime::CancelFlag,
    service_platform::{Launchd, ServiceSpec},
};
use tv_application::{Clock, Context};
use tv_domain::{Effect, Failure, PortResult, Status};

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
fn spec() -> ServiceSpec {
    ServiceSpec {
        label: "org.tv-control.fixture-installation".into(),
        executable: "/private/fixture bin/tv-control".into(),
        configuration: "/private/fixture/config.json".into(),
        working_directory: "/private/fixture/connection".into(),
        artifact: "/private/fixture/LaunchAgents/org.tv-control.fixture-installation.plist".into(),
        uid: 501,
    }
}
enum Outcome {
    Success,
    Nonzero,
    Failure(Failure),
    CancelAfter,
    ExpireAfter,
}
struct Mock<'a> {
    outcome: Outcome,
    calls: Mutex<Vec<Invocation>>,
    clock: &'a TestClock,
    cancel: &'a CancelFlag,
}
impl<'a> Mock<'a> {
    fn new(outcome: Outcome, clock: &'a TestClock, cancel: &'a CancelFlag) -> Self {
        Self {
            outcome,
            calls: Mutex::new(vec![]),
            clock,
            cancel,
        }
    }
}
impl Process for Mock<'_> {
    fn run(&self, invocation: Invocation, _: &Context<'_>) -> PortResult<Output> {
        self.calls.lock().unwrap().push(invocation);
        match self.outcome {
            Outcome::Success => Ok(Output {
                stdout: b"untrusted output SECRET_CANARY".to_vec(),
                success: true,
            }),
            Outcome::Nonzero => Ok(Output {
                stdout: b"SECRET_CANARY".to_vec(),
                success: false,
            }),
            Outcome::Failure(error) => Err(error),
            Outcome::CancelAfter => {
                self.cancel.cancel();
                Ok(Output {
                    stdout: vec![],
                    success: true,
                })
            }
            Outcome::ExpireAfter => {
                self.clock.0.store(1000, Ordering::SeqCst);
                Ok(Output {
                    stdout: vec![],
                    success: true,
                })
            }
        }
    }
}

#[test]
fn lifecycle_commands_are_exactly_one_typed_invocation_each() {
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let process = Mock::new(Outcome::Success, &clock, &cancel);
    let platform = Launchd { process: &process };
    let spec = spec();
    platform
        .bootstrap(&spec, &context(&clock, &cancel))
        .unwrap();
    platform
        .kickstart(&spec, &context(&clock, &cancel))
        .unwrap();
    platform.bootout(&spec, &context(&clock, &cancel)).unwrap();
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    let expected = [
        vec![
            "bootstrap".into(),
            "gui/501".into(),
            spec.artifact.as_os_str().to_owned(),
        ],
        vec![
            "kickstart".into(),
            "gui/501/org.tv-control.fixture-installation".into(),
        ],
        vec![
            "bootout".into(),
            "gui/501/org.tv-control.fixture-installation".into(),
        ],
    ];
    for (call, expected) in calls.iter().zip(expected) {
        assert_eq!(call.program, PathBuf::from("/bin/launchctl"));
        assert_eq!(call.args, expected);
        assert_eq!(
            call.env,
            BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())])
        );
        assert!(call.stdin.is_empty());
        assert!(call.mutating);
        assert!(!call
            .args
            .iter()
            .any(|arg| matches!(arg.to_str(), Some("-k" | "print" | "list" | "-x"))));
    }
}

#[test]
fn malformed_labels_paths_and_privileged_domains_never_dispatch() {
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let process = Mock::new(Outcome::Success, &clock, &cancel);
    let platform = Launchd { process: &process };
    for label in [
        "",
        "-option",
        "has space",
        "has/slash",
        "system:job",
        "job\nvalue",
        "jobé",
    ] {
        let mut bad = spec();
        bad.label = label.into();
        assert_eq!(
            platform
                .bootstrap(&bad, &context(&clock, &cancel))
                .unwrap_err()
                .code,
            "invalid_service_specification"
        );
    }
    let mut bad = spec();
    bad.label = "a".repeat(129);
    assert!(bad.render().is_err());
    for path in [
        "relative",
        "/private/../foreign",
        "/private/./file",
        "/private/control\nfile",
        "/private/nul\0file",
    ] {
        for field in 0..4 {
            let mut bad = spec();
            match field {
                0 => bad.executable = path.into(),
                1 => bad.configuration = path.into(),
                2 => bad.working_directory = path.into(),
                _ => bad.artifact = path.into(),
            }
            assert!(bad.render().is_err());
            assert!(platform.kickstart(&bad, &context(&clock, &cancel)).is_err());
        }
    }
    let mut bad = spec();
    bad.artifact = format!("/{}", "a".repeat(4096)).into();
    assert!(platform.bootout(&bad, &context(&clock, &cancel)).is_err());
    for uid in [0, u32::MAX] {
        let mut bad = spec();
        bad.uid = uid;
        let failure = platform
            .bootstrap(&bad, &context(&clock, &cancel))
            .unwrap_err();
        assert_eq!(failure.status(), Status::Unsupported);
    }
    assert!(process.calls.lock().unwrap().is_empty());
}

#[test]
fn non_utf8_paths_are_refused_without_dispatch() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let process = Mock::new(Outcome::Success, &clock, &cancel);
    let mut bad = spec();
    bad.configuration = OsString::from_vec(b"/private/\xff".to_vec()).into();
    assert_eq!(
        Launchd { process: &process }
            .bootout(&bad, &context(&clock, &cancel))
            .unwrap_err()
            .code,
        "invalid_service_specification"
    );
    assert!(process.calls.lock().unwrap().is_empty());
}

#[test]
fn cancellation_or_expiry_before_dispatch_writes_nothing() {
    for expired in [false, true] {
        let clock = TestClock::default();
        let cancel = CancelFlag::default();
        if expired {
            clock.0.store(1000, Ordering::SeqCst);
        } else {
            cancel.cancel();
        }
        let process = Mock::new(Outcome::Success, &clock, &cancel);
        let failure = Launchd { process: &process }
            .bootstrap(&spec(), &context(&clock, &cancel))
            .unwrap_err();
        assert_eq!(failure.effect, Effect::None);
        assert_eq!(
            failure.status(),
            if expired {
                Status::Expired
            } else {
                Status::Cancelled
            }
        );
        assert!(process.calls.lock().unwrap().is_empty());
    }
}

#[test]
fn one_dispatch_never_retries_or_turns_late_cancellation_into_success() {
    for outcome in [
        Outcome::Nonzero,
        Outcome::CancelAfter,
        Outcome::ExpireAfter,
        Outcome::Failure(Failure::uncertain("process_pipe_failed")),
    ] {
        let clock = TestClock::default();
        let cancel = CancelFlag::default();
        let process = Mock::new(outcome, &clock, &cancel);
        let failure = Launchd { process: &process }
            .bootstrap(&spec(), &context(&clock, &cancel))
            .unwrap_err();
        assert_eq!(failure.effect, Effect::MayHaveChanged);
        assert_eq!(failure.status(), Status::OutcomeUnknown);
        assert_eq!(process.calls.lock().unwrap().len(), 1);
        assert!(!format!("{failure:?}").contains("SECRET_CANARY"));
    }
}

#[test]
fn a_known_pre_spawn_refusal_retains_its_classification() {
    let clock = TestClock::default();
    let cancel = CancelFlag::default();
    let process = Mock::new(
        Outcome::Failure(Failure::before("process_spawn_failed").with_os_error(Some(libc::EACCES))),
        &clock,
        &cancel,
    );
    let failure = Launchd { process: &process }
        .kickstart(&spec(), &context(&clock, &cancel))
        .unwrap_err();
    assert_eq!(failure.effect, Effect::None);
    assert_eq!(failure.os_error, Some(libc::EACCES));
    assert_eq!(process.calls.lock().unwrap().len(), 1);
}

#[test]
fn renderer_is_bounded_and_contains_no_shell_credentials_or_path_watchers() {
    let mut spec = spec();
    spec.executable = "/private/a&b/<tool>/\"quoted\"/'launcher'".into();
    spec.configuration = "/private/$(not-a-shell);config.json".into();
    let bytes = spec.render().unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("a&amp;b/&lt;tool&gt;/&quot;quoted&quot;/&apos;launcher&apos;"));
    assert!(text.contains("$(not-a-shell);config.json"));
    for forbidden in [
        "EnvironmentVariables",
        "PathState",
        "WatchPaths",
        "NetworkState",
        "MachServices",
        "Sockets",
        "UserName",
        "sudo",
        "/bin/sh",
    ] {
        assert!(!text.contains(forbidden), "{forbidden}");
    }
    let escaped = format!("/{}", "\"".repeat(4095));
    spec.executable = escaped.clone().into();
    spec.configuration = escaped.clone().into();
    spec.working_directory = escaped.into();
    assert_eq!(spec.render().unwrap_err().code, "service_artifact_limit");
}

#[cfg(target_os = "macos")]
#[test]
fn macos_plutil_statically_validates_and_decodes_generated_artifact() {
    use std::{
        fs,
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
        process::Command,
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().canonicalize().unwrap().join(format!(
        "tv-service-plist-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    struct Remove(PathBuf);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _remove = Remove(directory.clone());
    let path = directory.join("agent.plist");
    let mut spec = spec();
    spec.executable = "/private/fixture<&>\"'/tv-control".into();
    let bytes = spec.render().unwrap();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    file.write_all(&bytes).unwrap();
    drop(file);
    let lint = Command::new("/usr/bin/plutil")
        .arg("-lint")
        .arg(&path)
        .output()
        .unwrap();
    assert!(lint.status.success(), "static plist lint failed");
    let output = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["ProgramArguments"],
        serde_json::json!([
            spec.executable.to_str().unwrap(),
            "service",
            "run",
            spec.configuration.to_str().unwrap()
        ])
    );
    assert_eq!(value["Label"], spec.label);
    assert_eq!(
        value["WorkingDirectory"],
        spec.working_directory.to_str().unwrap()
    );
    assert_eq!(
        value["KeepAlive"],
        serde_json::json!({"SuccessfulExit": false})
    );
    assert_eq!(value["RunAtLoad"], true);
    assert_eq!(value["ThrottleInterval"], 30);
    assert_eq!(value["ExitTimeOut"], 20);
    assert_eq!(value["Umask"], 63);
    assert_eq!(value["StandardOutPath"], "/dev/null");
    assert_eq!(value["StandardErrorPath"], "/dev/null");
    assert_eq!(fs::read(&path).unwrap(), bytes);
}
