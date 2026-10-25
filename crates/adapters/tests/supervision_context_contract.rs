//! Transaction contention must not replay a supervisor lifecycle action.
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File},
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tv_adapters::{
    runtime::CancelFlag,
    supervision_store::{Binding, ContextStore, SupervisionStore},
};
use tv_application::{
    supervision::{Record, Registration, Store},
    Clock, Context,
};
use tv_domain::{Effect, Failure, PortResult, Status};

type Hook = Box<dyn FnOnce() + Send>;
#[derive(Default)]
struct HookClock {
    now: AtomicU64,
    sleeps: AtomicUsize,
    hook: Mutex<Option<Hook>>,
}
impl Clock for HookClock {
    fn unix_ms(&self) -> u64 {
        100 + self.monotonic_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, ms: u64) {
        self.sleeps.fetch_add(1, Ordering::SeqCst);
        self.now.fetch_add(ms, Ordering::SeqCst);
        if let Some(hook) = self.hook.lock().unwrap().take() {
            hook();
        }
    }
}
fn context<'a>(clock: &'a dyn Clock, cancel: &'a CancelFlag) -> Context<'a> {
    Context {
        clock,
        cancellation: cancel,
        deadline_monotonic_ms: 1000,
    }
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> (Self, SupervisionStore) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "tv-supervision-context-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let b = Binding {
            installation_id: "fixture".into(),
            tunnel_id: "tunnel_fixture".into(),
            configuration: "/fixture/config".into(),
            launcher: "/fixture/launcher".into(),
            helper: "/fixture/helper".into(),
            artifact: "/fixture/agent.plist".into(),
            executable_path: "/usr/bin:/bin".into(),
            label: "org.tv-control.fixture".into(),
            uid: 501,
            pins: BTreeMap::from([("/fixture/key".into(), "a".repeat(64))]),
        };
        let store = SupervisionStore::prepare(&path, &b, true).unwrap();
        (Self(path), store)
    }
    fn hold_transaction(&self) -> HeldLock {
        let file = File::open(self.0.join("supervision/transaction.lock")).unwrap();
        assert_eq!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        HeldLock(file)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct HeldLock(File);
impl Drop for HeldLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
fn code<T>(value: PortResult<T>) -> Failure {
    match value {
        Err(error) => error,
        Ok(_) => panic!("expected refusal"),
    }
}
fn target() -> Record {
    let mut r = Record::prepared();
    r.revision = 1;
    r.registration = Registration::Installed;
    r
}

#[test]
fn real_contention_is_released_at_the_first_wait_without_recreating_or_writing_state() {
    for action in ["load", "binding", "open", "owner", "cas"] {
        let (temp, store) = Temp::new();
        let bytes = fs::read(temp.0.join("supervision/state.json")).unwrap();
        let held = temp.hold_transaction();
        assert_eq!(code(store.load()).code, "supervision_store_busy");
        let clock = HookClock::default();
        *clock.hook.lock().unwrap() = Some(Box::new(move || drop(held)));
        let cancel = CancelFlag::default();
        let ctx = context(&clock, &cancel);
        match action {
            "load" => assert_eq!(store.load_with_context(&ctx).unwrap(), Record::prepared()),
            "binding" => assert_eq!(
                store.binding_with_context(&ctx).unwrap().installation_id,
                "fixture"
            ),
            "open" => assert_eq!(
                SupervisionStore::open_with_context(&temp.0, &ctx)
                    .unwrap()
                    .load()
                    .unwrap(),
                Record::prepared()
            ),
            "owner" => drop(store.try_owner_with_context(&ctx).unwrap()),
            "cas" => assert!(store
                .with_context(&ctx)
                .compare_and_set(0, &target())
                .unwrap()),
            _ => unreachable!(),
        }
        assert_eq!(clock.sleeps.load(Ordering::SeqCst), 1);
        if action != "cas" {
            assert_eq!(
                fs::read(temp.0.join("supervision/state.json")).unwrap(),
                bytes
            );
        }
    }
}

struct ScriptStore {
    results: Mutex<VecDeque<PortResult<bool>>>,
    calls: Mutex<Vec<(u64, Record)>>,
    writes: AtomicUsize,
    loads: AtomicUsize,
    load_error: Option<Failure>,
    committed: Mutex<Option<Hook>>,
}
impl ScriptStore {
    fn new(results: impl IntoIterator<Item = PortResult<bool>>) -> Self {
        Self {
            results: Mutex::new(results.into_iter().collect()),
            calls: Mutex::new(vec![]),
            writes: AtomicUsize::new(0),
            loads: AtomicUsize::new(0),
            load_error: None,
            committed: Mutex::new(None),
        }
    }
}
impl Store for ScriptStore {
    fn load(&self) -> PortResult<Record> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.load_error.map_or_else(|| Ok(Record::prepared()), Err)
    }
    fn compare_and_set(&self, expected: u64, next: &Record) -> PortResult<bool> {
        self.calls.lock().unwrap().push((expected, next.clone()));
        let result = self
            .results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(Failure::before("supervision_store_busy")));
        if matches!(result, Ok(true))
            || result
                .as_ref()
                .err()
                .is_some_and(|e| e.effect == Effect::MayHaveChanged)
        {
            self.writes.fetch_add(1, Ordering::SeqCst);
            if let Some(hook) = self.committed.lock().unwrap().take() {
                hook();
            }
        }
        result
    }
}

#[test]
fn cas_retries_only_immutable_transaction_arguments_without_loading_or_replaying_actions() {
    let busy = Failure::before("supervision_store_busy");
    let store = ScriptStore::new([Err(busy), Err(busy), Ok(true)]);
    let clock = HookClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    let next = target();
    assert!(ContextStore::new(&store, &ctx)
        .compare_and_set(0, &next)
        .unwrap());
    assert_eq!(*store.calls.lock().unwrap(), vec![(0, next); 3]);
    assert_eq!(store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(store.loads.load(Ordering::SeqCst), 0);
}

#[test]
fn cancellation_or_expiry_before_or_during_wait_prevents_the_write() {
    for mode in [
        "cancel_before",
        "expire_before",
        "cancel_wait",
        "expire_wait",
    ] {
        let clock = Arc::new(HookClock::default());
        let cancel = Arc::new(CancelFlag::default());
        if mode == "cancel_before" {
            cancel.cancel();
        }
        if mode == "expire_before" {
            clock.now.store(1000, Ordering::SeqCst);
        }
        let busy = Failure::before("supervision_store_busy");
        let store = ScriptStore::new([Err(busy), Ok(true)]);
        if mode.ends_with("wait") {
            let hook_clock = clock.clone();
            let hook_cancel = cancel.clone();
            let hook: Hook = Box::new(move || {
                if mode == "cancel_wait" {
                    hook_cancel.cancel();
                } else {
                    hook_clock.now.store(1000, Ordering::SeqCst);
                }
            });
            *clock.hook.lock().unwrap() = Some(hook);
        }
        let ctx = context(clock.as_ref(), cancel.as_ref());
        let error = code(ContextStore::new(&store, &ctx).compare_and_set(0, &target()));
        assert_eq!(
            error.code,
            if mode.starts_with("cancel") {
                "cancelled"
            } else {
                "expired"
            }
        );
        assert_eq!(
            store.calls.lock().unwrap().len(),
            usize::from(mode.ends_with("wait"))
        );
        assert_eq!(store.writes.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn uncertain_cas_permissions_and_other_failures_are_not_retried() {
    for failure in [
        Failure::uncertain("supervision_store_busy"),
        Failure::before("supervision_store_busy").with_os_error(Some(libc::EACCES)),
        Failure::before("supervision_store_busy").with_os_error(Some(libc::EPERM)),
        Failure::cancelled("supervision_store_busy"),
        Failure::expired("supervision_store_busy"),
        Failure::before("supervision_store_corrupt"),
        Failure::before("supervision_store_io"),
        Failure::before("supervision_owner_busy"),
    ] {
        let store = ScriptStore::new([Err(failure), Ok(true)]);
        let clock = HookClock::default();
        let cancel = CancelFlag::default();
        let ctx = context(&clock, &cancel);
        assert_eq!(
            code(ContextStore::new(&store, &ctx).compare_and_set(0, &target())),
            failure
        );
        assert_eq!(store.calls.lock().unwrap().len(), 1);
        assert_eq!(clock.sleeps.load(Ordering::SeqCst), 0);
        assert_eq!(
            store.writes.load(Ordering::SeqCst),
            usize::from(failure.effect == Effect::MayHaveChanged)
        );
    }
}

#[test]
fn successful_commit_is_not_relabelled_as_a_before_dispatch_cancellation() {
    let store = ScriptStore::new([Ok(true)]);
    let cancel = Arc::new(CancelFlag::default());
    let committed_cancel = cancel.clone();
    *store.committed.lock().unwrap() = Some(Box::new(move || committed_cancel.cancel()));
    let clock = HookClock::default();
    let ctx = context(&clock, cancel.as_ref());
    assert!(ContextStore::new(&store, &ctx)
        .compare_and_set(0, &target())
        .unwrap());
    assert_eq!(store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(code(ctx.check()).code, "cancelled");
}

#[test]
fn owner_busy_and_corrupt_real_state_are_not_waited_or_repaired() {
    let (temp, store) = Temp::new();
    let _owner = store.try_owner().unwrap();
    let clock = HookClock::default();
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    assert_eq!(
        code(store.try_owner_with_context(&ctx)).code,
        "supervision_owner_busy"
    );
    fs::write(temp.0.join("supervision/state.json"), b"invalid").unwrap();
    assert_eq!(
        code(store.load_with_context(&ctx)).code,
        "supervision_store_corrupt"
    );
    assert_eq!(
        code(SupervisionStore::open_with_context(&temp.0, &ctx)).code,
        "supervision_store_corrupt"
    );
    assert_eq!(clock.sleeps.load(Ordering::SeqCst), 0);
    assert_eq!(
        fs::read(temp.0.join("supervision/state.json")).unwrap(),
        b"invalid"
    );
}

#[test]
fn real_time_ceiling_bounds_a_stalled_application_clock_and_returns_original_busy() {
    struct FrozenClock(Instant);
    impl Clock for FrozenClock {
        fn unix_ms(&self) -> u64 {
            100
        }
        fn monotonic_ms(&self) -> u64 {
            0
        }
        fn sleep_ms(&self, _: u64) {
            // The test itself fails finitely if the real-time guard regresses.
            assert!(
                self.0.elapsed() < Duration::from_millis(500),
                "stalled retry exceeded its real-time bound"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let busy = Failure::before("supervision_store_busy").with_os_error(Some(libc::EWOULDBLOCK));
    struct Busy(Failure);
    impl Store for Busy {
        fn load(&self) -> PortResult<Record> {
            Err(self.0)
        }
        fn compare_and_set(&self, _: u64, _: &Record) -> PortResult<bool> {
            Err(self.0)
        }
    }
    let clock = FrozenClock(Instant::now());
    let cancel = CancelFlag::default();
    let ctx = context(&clock, &cancel);
    let error = code(ContextStore::new(&Busy(busy), &ctx).load());
    assert_eq!(error, busy);
    assert_eq!(error.status(), Status::Failed);
    assert!(clock.0.elapsed() < Duration::from_millis(500));
}
