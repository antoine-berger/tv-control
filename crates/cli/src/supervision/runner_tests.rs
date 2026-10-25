//! Composition tests use private stores and directly owned processes only.
use super::*;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
use tv_application::supervision::{Backend, Observation};

const CHILD_TEST: &str = "supervision::runner::tests::isolated_runner";
const CHILD_ENV: &str = "TV_CONTROL_RUNNER_TEST_CHILD";

fn eventually(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < until, "owned fixture phase timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn retry_busy<T>(mut action: impl FnMut() -> PortResult<T>) -> T {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match action() {
            Ok(value) => return value,
            Err(error) if error.code == "supervision_store_busy" && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("fixture store access failed: {error:?}"),
        }
    }
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "tv-runner-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let store = SupervisionStore::prepare(
            &path,
            &Binding {
                installation_id: "fixture_installation".into(),
                tunnel_id: "tunnel_fixture".into(),
                configuration: path.join("config.json"),
                launcher: path.join("launcher"),
                helper: path.join("helper"),
                executable_path: "/usr/bin:/bin".into(),
                label: "org.tv-control.fixture".into(),
                artifact: path.join("agent.plist"),
                // Only schema data: this fixture never calls an OS service
                // manager. Store ownership is checked against the real UID.
                uid: 501,
                pins: BTreeMap::from([(
                    path.join("identity").to_str().unwrap().into(),
                    "a".repeat(64),
                )]),
            },
            true,
        )
        .unwrap();
        let mut record = store.load().unwrap();
        record.registration = Registration::Installed;
        record.revision += 1;
        assert!(store.compare_and_set(0, &record).unwrap());
        let clock = SystemClock::default();
        let cancel = CancelFlag::default();
        let ctx = Context {
            clock: &clock,
            cancellation: &cancel,
            deadline_monotonic_ms: 5000,
        };
        policy::rearm(&store, &ctx).unwrap();
        Self(path)
    }
    fn store(&self) -> SupervisionStore {
        retry_busy(|| SupervisionStore::open(&self.0))
    }
    fn record(&self) -> Record {
        retry_busy(|| self.store().load())
    }
    fn events(&self) -> String {
        fs::read_to_string(self.0.join("events")).unwrap_or_default()
    }
    fn launch(&self, mode: &str) -> Owned {
        Owned(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", CHILD_TEST, "--nocapture"])
                .env(CHILD_ENV, mode)
                .env("TV_CONTROL_RUNNER_TEST_DIRECTORY", &self.0)
                .stdin(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }
    fn ready(&self, child: &mut Owned) {
        eventually(|| {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "runner exited before Ready; events={}",
                self.events()
            );
            self.0.join("ready").exists()
        });
        assert_eq!(self.record().code, Code::Ready);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Owned(Child);
impl Owned {
    fn exit(&mut self, expected: i32) {
        let mut status = None;
        eventually(|| {
            status = self.0.try_wait().unwrap();
            status.is_some()
        });
        assert_eq!(status.unwrap().code(), Some(expected));
    }
    fn signal(&mut self, signal: i32) {
        assert!(self.0.try_wait().unwrap().is_none());
        // The handle is a live, unreaped child created by this fixture.
        assert_eq!(unsafe { libc::kill(self.0.id() as i32, signal) }, 0);
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[derive(Default)]
struct FastClock(AtomicU64);
impl Clock for FastClock {
    fn unix_ms(&self) -> u64 {
        self.monotonic_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
    fn sleep_ms(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::Relaxed);
    }
}

// An explicit phase fence avoids making the scenario depend on winning a
// nonblocking store lock against the runner's admission. This is an injected
// clock only; no production flag or wall-clock timeout is changed.
struct GateClock {
    inner: SystemClock,
    directory: PathBuf,
    first_sleep: AtomicBool,
}
impl Clock for GateClock {
    fn unix_ms(&self) -> u64 {
        self.inner.unix_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.inner.monotonic_ms()
    }
    fn sleep_ms(&self, ms: u64) {
        // The transaction facade may also sleep before admission. Only the
        // completed Ready publication is the test's launcher/signal fence.
        if !self.first_sleep.load(Ordering::Relaxed)
            && SupervisionStore::open(&self.directory)
                .and_then(|store| store.load())
                .is_ok_and(|record| record.code == Code::Ready)
            && !self.first_sleep.swap(true, Ordering::Relaxed)
        {
            fs::write(self.directory.join("ready"), []).unwrap();
            eventually(|| self.directory.join("release-ready").exists());
        }
        self.inner.sleep_ms(ms);
    }
}

fn event(directory: &Path, value: &str) {
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(directory.join("events"))
        .unwrap();
    writeln!(file, "{value}").unwrap();
}

// Faults are at the Store port, not a replay of tick or a special production
// backend. Busy is produced by the real store under a real transaction flock.
struct ContendedStore<'a> {
    inner: &'a SupervisionStore,
    mode: &'a str,
    first_load: AtomicBool,
    cas_attempts: AtomicU64,
    pending: Mutex<Option<(u64, Record)>>,
}
impl ContendedStore<'_> {
    fn locked<T>(&self, operation: impl FnOnce() -> PortResult<T>) -> PortResult<T> {
        struct Lock(std::fs::File);
        impl Drop for Lock {
            fn drop(&mut self) {
                assert_eq!(unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) }, 0);
            }
        }
        let lock = Lock(
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(self.inner.directory().join("transaction.lock"))
                .unwrap(),
        );
        assert_eq!(
            unsafe { libc::flock(lock.0.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        let result = operation();
        assert!(
            matches!(&result, Err(error) if error.code == "supervision_store_busy" && error.effect == tv_domain::Effect::None)
        );
        result
    }
}
impl Store for ContendedStore<'_> {
    fn load(&self) -> PortResult<Record> {
        if self.mode == "busy_entry" && !self.first_load.swap(true, Ordering::Relaxed) {
            event(self.inner.directory().parent().unwrap(), "entry_busy");
            return self.locked(|| self.inner.load());
        }
        let record = self.inner.load()?;
        if matches!(
            self.mode,
            "cancel_before_start" | "cancel_after_start" | "cancel_cleanup_unknown"
        ) && record.phase == Phase::StartInFlight
        {
            let started =
                fs::read_to_string(self.inner.directory().parent().unwrap().join("events"))
                    .unwrap_or_default()
                    .lines()
                    .any(|line| line == "start");
            if started == (self.mode != "cancel_before_start")
                && !self.first_load.swap(true, Ordering::Relaxed)
            {
                event(self.inner.directory().parent().unwrap(), "cancelled_read");
                // This Store is used only inside the isolated owned test child,
                // whose production Interrupts guard has installed its handler.
                assert_eq!(unsafe { libc::raise(libc::SIGHUP) }, 0);
            }
        }
        Ok(record)
    }
    fn compare_and_set(&self, expected: u64, next: &Record) -> PortResult<bool> {
        if next.code == Code::Ready
            && matches!(
                self.mode,
                "busy_after_start"
                    | "busy_sticky_after_start"
                    | "unknown_store"
                    | "permission_store"
                    | "corrupt_store"
            )
        {
            event(self.inner.directory().parent().unwrap(), "ready_cas");
            let attempt = self.cas_attempts.fetch_add(1, Ordering::Relaxed);
            let mut pending = self.pending.lock().unwrap();
            if let Some((revision, record)) = &*pending {
                assert_eq!(
                    (expected, next),
                    (*revision, record),
                    "CAS retry changed its immutable target"
                );
            } else {
                *pending = Some((expected, next.clone()));
            }
            drop(pending);
            match self.mode {
                "busy_after_start" if attempt == 0 => {
                    return self.locked(|| self.inner.compare_and_set(expected, next))
                }
                "busy_sticky_after_start" => {
                    return self.locked(|| self.inner.compare_and_set(expected, next))
                }
                "unknown_store" => return Err(Failure::uncertain("supervision_store_busy")),
                "permission_store" => return Err(Failure::before("permission_denied")),
                "corrupt_store" => return Err(Failure::before("supervision_state_invalid")),
                _ => (),
            }
        }
        self.inner.compare_and_set(expected, next)
    }
}

struct FakeBackend {
    directory: PathBuf,
    mode: String,
    child: Mutex<Option<Child>>,
}
impl FakeBackend {
    fn event(&self, value: &str) {
        event(&self.directory, value);
    }
    fn finish_child(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            // Closing only our pipe releases the owned shell's read. No process
            // lookup, historical PID or external server is involved.
            drop(child.stdin.take());
            let until = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    self.event("child_reaped");
                    return;
                }
                if Instant::now() >= until {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("owned controller did not finish after pipe closure");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
impl Drop for FakeBackend {
    fn drop(&mut self) {
        self.finish_child();
    }
}
impl Backend for FakeBackend {
    fn observe_owned(&self, ctx: &Context<'_>) -> PortResult<Observation> {
        ctx.check()?;
        self.event("observe");
        if self.mode == "stop_gate" && self.child.lock().unwrap().is_some() {
            self.event("observe-held");
            while !self.directory.join("allow-observe").exists() {
                ctx.wait(5)?;
            }
        }
        if self.mode == "crash" {
            // A completed Ready tick is on disk before the next observation.
            if self.child.lock().unwrap().is_some() {
                assert_eq!(
                    retry_busy(|| SupervisionStore::open(&self.directory)?.load()).code,
                    Code::Ready
                );
                self.finish_child();
            }
        }
        Ok(if self.child.lock().unwrap().is_some() {
            Observation::Ready
        } else {
            Observation::Absent
        })
    }
    fn start_owned(&self, ctx: &Context<'_>) -> PortResult<Observation> {
        ctx.check()?;
        self.event("start");
        if self.mode == "unknown" {
            return Err(Failure::uncertain("fixture_start_unknown"));
        }
        if self.mode == "refuse" {
            return Err(Failure::before("fixture_start_refused"));
        }
        let marker = self
            .directory
            .join(format!("finished-{}", std::process::id()));
        // Builtins only: exactly one directly owned process. EOF also permits
        // it to finish if its fixture parent is forcibly terminated on failure.
        let child = Command::new("/bin/sh")
            .args([
                "-c",
                "IFS= read -r fixture_line; printf 'finished\\n' > \"$1\"",
                "fixture",
            ])
            .arg(marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(self.child.lock().unwrap().replace(child).is_none());
        Ok(Observation::Ready)
    }
    fn stop_owned(&self, ctx: &Context<'_>) -> PortResult<Observation> {
        ctx.check()?;
        self.event("stop");
        if self.mode == "cancel_cleanup_unknown" {
            // Fixture Drop still reaps its own process. That teardown is not
            // an acknowledgement of the policy's uncertain cleanup operation.
            return Err(Failure::uncertain("fixture_cleanup_unknown"));
        }
        if self.mode == "stop_gate" {
            while !self.directory.join("allow-stop").exists() {
                ctx.wait(5)?;
            }
        }
        self.finish_child();
        ctx.check()?;
        Ok(Observation::Absent)
    }
}

#[test]
fn isolated_runner() {
    let Ok(mode) = std::env::var(CHILD_ENV) else {
        return;
    };
    let directory = PathBuf::from(std::env::var_os("TV_CONTROL_RUNNER_TEST_DIRECTORY").unwrap());
    let code = {
        let interrupts = Interrupts::install().unwrap();
        let store = retry_busy(|| SupervisionStore::open(&directory));
        let _lease = store.try_owner().unwrap();
        let fast = FastClock::default();
        let real = SystemClock::default();
        let gate = GateClock {
            inner: SystemClock::default(),
            directory: directory.clone(),
            first_sleep: AtomicBool::new(false),
        };
        let clock: &dyn Clock = match mode.as_str() {
            "crash" => &fast,
            "hold" | "stop_gate" | "busy_entry" | "busy_after_start" => &gate,
            _ => &real,
        };
        let ctx = Context {
            clock,
            cancellation: &interrupts,
            deadline_monotonic_ms: 5000,
        };
        let injected = ContendedStore {
            inner: &store,
            mode: &mode,
            first_load: AtomicBool::new(false),
            cas_attempts: AtomicU64::new(0),
            pending: Mutex::new(None),
        };
        restart_on_contention(
            (|| match policy::enter(&ContextStore::new(&injected, &ctx), &ctx)? {
                policy::Entry::Owner(mut owner) => {
                    let mut attempts = 0;
                    drive_admitted(
                        &injected,
                        clock,
                        &interrupts,
                        &mut owner,
                        &ctx,
                        |construction| {
                            if mode.starts_with("constructor_") {
                                attempts += 1;
                                event(&directory, "construct");
                                if attempts == 1 && mode.starts_with("constructor_cancel") {
                                    assert_eq!(unsafe { libc::raise(libc::SIGHUP) }, 0);
                                }
                                construction.check()?;
                                if mode.ends_with("permission") {
                                    return Err(Failure::before("permission_denied"));
                                }
                                if mode.ends_with("unknown") {
                                    return Err(Failure::uncertain("fixture_constructor_unknown"));
                                }
                            }
                            Ok(FakeBackend {
                                directory: directory.clone(),
                                mode: mode.clone(),
                                child: Mutex::new(None),
                            })
                        },
                    )
                }
                _ => Ok(0),
            })(),
        )
        .unwrap_or(0)
    }; // All owned child handles, signal guards and leases drop before exit.
    std::process::exit(code);
}

#[test]
fn launcher_stdin_eof_does_not_stop_runner_and_stop_latch_bypasses_owner() {
    let fixture = Fixture::new();
    let mut child = fixture.launch("stop_gate");
    fixture.ready(&mut child);
    drop(child.0.stdin.take());
    fs::write(fixture.0.join("release-ready"), []).unwrap();
    // A subsequent backend observation is entered after launcher EOF. Its
    // explicit fence keeps the owner live while another process publishes Stop.
    eventually(|| fixture.events().lines().any(|line| line == "observe-held"));
    assert!(child.0.try_wait().unwrap().is_none());
    let store = fixture.store();
    assert!(matches!(store.try_owner(), Err(e) if e.code == "supervision_owner_busy"));
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = Context {
        clock: &clock,
        cancellation: &cancel,
        deadline_monotonic_ms: 5000,
    };
    retry_busy(|| policy::request_stop(&store, &ctx));
    assert_eq!(store.load().unwrap().mode, Mode::Stopped);
    fs::write(fixture.0.join("allow-observe"), []).unwrap();
    eventually(|| fixture.events().lines().any(|line| line == "stop"));
    // Publication returned while the old owner still holds its lease and its
    // controlled cleanup is deliberately waiting for this fixture's release.
    assert!(matches!(store.try_owner(), Err(e) if e.code == "supervision_owner_busy"));
    assert!(child.0.try_wait().unwrap().is_none());
    fs::write(fixture.0.join("allow-stop"), []).unwrap();
    child.exit(0);
    let stopped = fixture.record();
    assert_eq!(
        (stopped.mode, stopped.phase, stopped.code),
        (Mode::Stopped, Phase::Idle, Code::CleanupVerified)
    );
    assert_eq!(
        fixture
            .events()
            .lines()
            .filter(|line| *line == "child_reaped")
            .count(),
        1
    );
    let before = fixture.events();
    fixture.launch("stop_gate").exit(0);
    assert_eq!(
        fixture.events(),
        before,
        "Stopped incarnation called backend"
    );
}

#[test]
fn hangup_preserves_enabled_intent_and_restart_budget_survives_incarnations() {
    let fixture = Fixture::new();
    let mut child = fixture.launch("hold");
    fixture.ready(&mut child);
    assert_eq!(fixture.record().starts, 1);
    child.signal(libc::SIGHUP);
    fs::write(fixture.0.join("release-ready"), []).unwrap();
    child.exit(1);
    let after_hup = fixture.record();
    assert_eq!(
        (after_hup.mode, after_hup.phase, after_hup.code),
        (Mode::Enabled, Phase::Idle, Code::CleanupVerified)
    );
    assert_eq!(after_hup.starts, 1);
    assert_eq!(
        fixture
            .events()
            .lines()
            .filter(|line| *line == "child_reaped")
            .count(),
        1
    );
    // A new process with a new monotonic origin must retain the first start.
    fixture.launch("crash").exit(0);
    let exhausted = fixture.record();
    assert_eq!(
        (exhausted.mode, exhausted.code, exhausted.starts),
        (Mode::Blocked, Code::StartBudgetExhausted, 3)
    );
    assert_eq!(
        fixture
            .events()
            .lines()
            .filter(|line| *line == "start")
            .count(),
        3
    );
    assert_eq!(
        fixture
            .events()
            .lines()
            .filter(|line| *line == "child_reaped")
            .count(),
        3
    );
    let before = fixture.events();
    fixture.launch("crash").exit(0);
    assert_eq!(
        fixture.events(),
        before,
        "Blocked incarnation restarted controller"
    );
    assert_eq!(fixture.record(), exhausted);
}

#[test]
fn unknown_or_refused_start_never_loops_cleans_up_or_replays() {
    for (mode, phase, code) in [
        ("unknown", Phase::StartInFlight, Code::StartUncertain),
        ("refuse", Phase::Idle, Code::BackendRefused),
    ] {
        let fixture = Fixture::new();
        fixture.launch(mode).exit(0);
        let record = fixture.record();
        assert_eq!(
            (record.mode, record.phase, record.code, record.starts),
            (Mode::Blocked, phase, code, 1)
        );
        assert_eq!(fixture.events(), "observe\nstart\n");
        fixture.launch(mode).exit(0);
        assert_eq!(fixture.events(), "observe\nstart\n");
        assert_eq!(fixture.record(), record);
    }
}

#[test]
fn transaction_contention_retries_only_the_same_store_operation_not_the_backend() {
    for mode in ["busy_entry", "busy_after_start"] {
        let fixture = Fixture::new();
        let mut child = fixture.launch(mode);
        fixture.ready(&mut child);
        assert_eq!(
            fixture
                .events()
                .lines()
                .filter(|line| *line == "start")
                .count(),
            1
        );
        let attempts = fixture
            .events()
            .lines()
            .filter(|line| *line == "ready_cas")
            .count();
        assert_eq!(attempts, if mode == "busy_after_start" { 2 } else { 0 });
        let clock = SystemClock::default();
        let cancel = CancelFlag::default();
        let ctx = Context {
            clock: &clock,
            cancellation: &cancel,
            deadline_monotonic_ms: 5000,
        };
        policy::request_stop(&fixture.store(), &ctx).unwrap();
        fs::write(fixture.0.join("release-ready"), []).unwrap();
        child.exit(0);
        assert_eq!(fixture.record().code, Code::CleanupVerified);
        assert_eq!(
            fixture
                .events()
                .lines()
                .filter(|line| *line == "start")
                .count(),
            1
        );
    }
}

#[test]
fn exhausted_contention_or_unsafe_store_errors_leave_pending_start_unreplayed() {
    for (mode, exit) in [
        ("busy_sticky_after_start", 1),
        ("unknown_store", 0),
        ("permission_store", 0),
        ("corrupt_store", 0),
    ] {
        let fixture = Fixture::new();
        fixture.launch(mode).exit(exit);
        let pending = fixture.record();
        assert_eq!(
            (pending.mode, pending.phase, pending.starts),
            (Mode::Enabled, Phase::StartInFlight, 1)
        );
        let events = fixture.events();
        assert_eq!(events.lines().filter(|line| *line == "start").count(), 1);
        assert!(!events.lines().any(|line| line == "stop"));
        let attempts = events.lines().filter(|line| *line == "ready_cas").count();
        if mode == "busy_sticky_after_start" {
            assert!(attempts > 1);
        } else {
            assert_eq!(attempts, 1);
        }
        fixture.launch(mode).exit(0);
        assert_eq!(
            fixture.events(),
            events,
            "next incarnation replayed pending lifecycle"
        );
        let blocked = fixture.record();
        assert_eq!(
            (blocked.mode, blocked.phase, blocked.code, blocked.starts),
            (
                Mode::Blocked,
                Phase::StartInFlight,
                Code::InterruptedLifecycle,
                1
            )
        );
    }
}

#[test]
fn only_operational_pretransaction_contention_requests_os_retry() {
    let busy = "supervision_store_busy";
    assert_eq!(restart_on_contention(Err(Failure::before(busy))), Ok(1));
    for error in [
        Failure::uncertain(busy),
        Failure::cancelled(busy),
        Failure::expired(busy),
        Failure::unsupported(busy),
        Failure::before(busy).with_os_error(Some(libc::EACCES)),
        Failure::before(busy).with_os_error(Some(libc::EPERM)),
        Failure::before("supervision_store_changed"),
        Failure::before("permission_denied"),
    ] {
        assert_eq!(restart_on_contention(Err(error)), Err(error));
    }
}

fn assert_cancelled_store_read_cleanup(mode: &str, starts: usize) {
    let fixture = Fixture::new();
    fixture.launch(mode).exit(1);
    let record = fixture.record();
    assert_eq!(
        (record.mode, record.phase, record.code, record.starts),
        (Mode::Enabled, Phase::Idle, Code::CleanupVerified, 1)
    );
    let events = fixture.events();
    assert_eq!(
        events
            .lines()
            .filter(|line| *line == "cancelled_read")
            .count(),
        1
    );
    assert_eq!(
        events.lines().filter(|line| *line == "start").count(),
        starts
    );
    assert_eq!(events.lines().filter(|line| *line == "stop").count(), 1);
    assert_eq!(
        events
            .lines()
            .filter(|line| *line == "child_reaped")
            .count(),
        starts
    );
}

#[test]
fn cancellation_from_store_read_after_admission_uses_fresh_cleanup_context() {
    assert_cancelled_store_read_cleanup("cancel_before_start", 0);
}

#[test]
fn cancellation_from_store_read_after_start_uses_fresh_cleanup_context() {
    assert_cancelled_store_read_cleanup("cancel_after_start", 1);
}

#[test]
fn cancellation_with_unknown_cleanup_keeps_pending_phase_and_never_replays() {
    let fixture = Fixture::new();
    fixture.launch("cancel_cleanup_unknown").exit(0);
    let pending = fixture.record();
    assert_eq!(
        (pending.mode, pending.phase, pending.code, pending.starts),
        (
            Mode::Blocked,
            Phase::CleanupInFlight,
            Code::CleanupUnverified,
            1
        )
    );
    let events = fixture.events();
    assert_eq!(events.lines().filter(|line| *line == "start").count(), 1);
    assert_eq!(events.lines().filter(|line| *line == "stop").count(), 1);
    fixture.launch("cancel_cleanup_unknown").exit(0);
    assert_eq!(fixture.events(), events);
    assert_eq!(fixture.record(), pending);
}

#[test]
fn cancellation_after_owner_admission_rebuilds_only_read_only_backend_for_cleanup() {
    let fixture = Fixture::new();
    fixture.launch("constructor_cancel").exit(1);
    let record = fixture.record();
    assert_eq!(
        (record.mode, record.phase, record.code, record.starts),
        (Mode::Enabled, Phase::Idle, Code::CleanupVerified, 0)
    );
    assert_eq!(fixture.events(), "construct\nconstruct\nstop\n");
}

#[test]
fn constructor_refusals_or_unknown_never_claim_cleanup_or_retry_lifecycle() {
    for (mode, attempts) in [
        ("constructor_permission", 1),
        ("constructor_unknown", 1),
        ("constructor_cancel_permission", 2),
        ("constructor_cancel_unknown", 2),
    ] {
        let fixture = Fixture::new();
        fixture.launch(mode).exit(0);
        let record = fixture.record();
        assert_eq!(
            (record.mode, record.phase, record.code, record.starts),
            (Mode::Enabled, Phase::Monitoring, Code::Monitoring, 0)
        );
        assert_eq!(fixture.events(), "construct\n".repeat(attempts));
    }
}
