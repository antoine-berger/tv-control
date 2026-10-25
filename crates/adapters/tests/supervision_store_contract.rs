use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Barrier,
    },
};
use tv_adapters::supervision_store::{Binding, SupervisionStore};
use tv_application::supervision::{Code, Mode, Phase, Record, Registration, Store};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "tv-supervision-store-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn prepare(&self) -> SupervisionStore {
        SupervisionStore::prepare(&self.0, &binding(), true).unwrap()
    }
    fn open(&self) -> SupervisionStore {
        SupervisionStore::open(&self.0).unwrap()
    }
    fn state(&self) -> PathBuf {
        self.0.join("supervision/state.json")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn binding() -> Binding {
    Binding {
        installation_id: "fixture_installation".into(),
        tunnel_id: "tunnel_fixture".into(),
        configuration: "/fixture/config.json".into(),
        launcher: "/fixture/tv-control".into(),
        helper: "/fixture/tunnel-client".into(),
        executable_path: "/usr/bin:/bin".into(),
        label: "org.tv-control.fixture".into(),
        artifact: "/fixture/agent.plist".into(),
        uid: 501,
        pins: BTreeMap::from([("/fixture/identity".into(), "a".repeat(64))]),
    }
}
fn installed(store: &SupervisionStore) -> Record {
    let mut record = store.load().unwrap();
    record.revision += 1;
    record.registration = Registration::Installed;
    assert!(store.compare_and_set(record.revision - 1, &record).unwrap());
    record
}
fn enabled(store: &SupervisionStore) -> Record {
    let mut record = installed(store);
    record.revision += 1;
    record.arm += 1;
    record.mode = Mode::Enabled;
    record.code = Code::Armed;
    assert!(store.compare_and_set(record.revision - 1, &record).unwrap());
    record
}
fn code<T>(result: tv_domain::PortResult<T>) -> &'static str {
    match result {
        Err(error) => error.code,
        Ok(_) => panic!("expected refusal"),
    }
}

#[test]
fn missing_state_and_unapproved_prepare_never_create_or_adopt() {
    let temp = Temp::new();
    assert_eq!(
        code(SupervisionStore::open(&temp.0)),
        "supervision_store_missing"
    );
    assert_eq!(
        code(SupervisionStore::prepare(&temp.0, &binding(), false)),
        "supervision_approval_required"
    );
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 0);
    let store = temp.prepare();
    assert_eq!(store.load().unwrap(), Record::prepared());
    let before = fs::read(temp.state()).unwrap();
    assert_eq!(
        code(SupervisionStore::prepare(&temp.0, &binding(), true)),
        "supervision_store_exists"
    );
    assert_eq!(fs::read(temp.state()).unwrap(), before);
    for name in ["state.json", "transaction.lock", "owner.lock"] {
        assert_eq!(
            fs::metadata(temp.0.join("supervision").join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o600
        );
    }
}

#[test]
fn counters_and_inflight_uncertainty_survive_reopen_and_conflicts_do_not_write() {
    let temp = Temp::new();
    let store = temp.prepare();
    let mut next = enabled(&store);
    next.revision += 1;
    next.starts = 1;
    next.phase = Phase::StartInFlight;
    next.code = Code::Starting;
    assert!(store.compare_and_set(next.revision - 1, &next).unwrap());
    drop(store);
    let store = temp.open();
    assert_eq!(store.load().unwrap(), next);
    let bytes = fs::read(temp.state()).unwrap();
    let mut stale = next.clone();
    stale.revision = 1;
    assert!(!store.compare_and_set(0, &stale).unwrap());
    assert_eq!(fs::read(temp.state()).unwrap(), bytes);
    let mut rollback = next.clone();
    rollback.revision += 1;
    rollback.starts = 0;
    assert_eq!(
        code(store.compare_and_set(next.revision, &rollback)),
        "supervision_state_rollback_refused"
    );
    let mut unknown = next.clone();
    unknown.revision += 1;
    unknown.mode = Mode::Blocked;
    unknown.code = Code::StartUncertain;
    assert!(store.compare_and_set(next.revision, &unknown).unwrap());
    drop(store);
    assert_eq!(temp.open().load().unwrap(), unknown);
}

#[test]
fn competing_cas_publish_once_and_owner_lease_does_not_block_stop_publication() {
    let temp = Temp::new();
    let store = temp.prepare();
    let current = enabled(&store);
    let lease = store.try_owner().unwrap();
    let other = temp.open();
    assert_eq!(code(other.try_owner()), "supervision_owner_busy");
    let mut stopped = current.clone();
    stopped.revision += 1;
    stopped.mode = Mode::Stopped;
    stopped.code = Code::Stopped;
    assert!(other.compare_and_set(current.revision, &stopped).unwrap());
    assert_eq!(store.load().unwrap().mode, Mode::Stopped);
    drop(lease);
    let next_lease = other.try_owner().unwrap();
    drop(next_lease);

    let barrier = Arc::new(Barrier::new(3));
    let expected = stopped.revision;
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let store = temp.open();
            let barrier = barrier.clone();
            let mut next = stopped.clone();
            next.revision += 1;
            next.starts += 1;
            std::thread::spawn(move || {
                barrier.wait();
                store.compare_and_set(expected, &next)
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| matches!(r, Ok(true))).count(), 1);
    assert!(results.iter().all(|r| r.is_ok()
        || r.as_ref()
            .err()
            .is_some_and(|e| e.code == "supervision_store_busy")));
    assert_eq!(store.load().unwrap().starts, stopped.starts + 1);
}

#[test]
fn malformed_unknown_or_over_budget_state_is_not_repaired() {
    for mutation in ["schema", "unknown", "code", "counter", "large"] {
        let temp = Temp::new();
        let store = temp.prepare();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.state()).unwrap()).unwrap();
        match mutation {
            "schema" => value["schema"] = 2.into(),
            "unknown" => value["unexpected"] = true.into(),
            "code" => value["record"]["code"] = "future_unknown".into(),
            "counter" => value["record"]["starts"] = 4.into(),
            _ => {}
        }
        let bytes = if mutation == "large" {
            vec![b' '; 128 * 1024 + 1]
        } else {
            serde_json::to_vec(&value).unwrap()
        };
        fs::write(temp.state(), &bytes).unwrap();
        assert_eq!(
            code(store.load()),
            if mutation == "large" {
                "supervision_store_limit"
            } else {
                "supervision_store_corrupt"
            }
        );
        assert_eq!(fs::read(temp.state()).unwrap(), bytes);
    }
}

#[test]
fn private_file_and_directory_bindings_refuse_links_and_replacements() {
    for mutation in [
        "mode",
        "hardlink",
        "symlink",
        "directory",
        "transaction",
        "binding",
    ] {
        let temp = Temp::new();
        let store = temp.prepare();
        let state = temp.state();
        match mutation {
            "mode" => fs::set_permissions(&state, fs::Permissions::from_mode(0o644)).unwrap(),
            "hardlink" => fs::hard_link(&state, temp.0.join("extra")).unwrap(),
            "symlink" => {
                fs::remove_file(&state).unwrap();
                fs::write(temp.0.join("foreign"), b"unchanged").unwrap();
                symlink(temp.0.join("foreign"), &state).unwrap();
            }
            "directory" => {
                fs::rename(temp.0.join("supervision"), temp.0.join("old")).unwrap();
                fs::create_dir(temp.0.join("supervision")).unwrap();
                fs::set_permissions(
                    temp.0.join("supervision"),
                    fs::Permissions::from_mode(0o700),
                )
                .unwrap();
            }
            "transaction" => {
                let path = temp.0.join("supervision/transaction.lock");
                fs::remove_file(&path).unwrap();
                fs::write(&path, b"").unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            "binding" => {
                let mut value: serde_json::Value =
                    serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
                value["binding"]["installation_id"] = "replacement".into();
                fs::write(&state, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(store.load().is_err(), "{mutation}");
        if mutation == "symlink" {
            assert_eq!(fs::read(temp.0.join("foreign")).unwrap(), b"unchanged");
        }
    }
}

#[test]
fn invalid_binding_is_refused_before_preparation() {
    let temp = Temp::new();
    for field in ["pin", "path", "label", "credential", "uid"] {
        let mut b = binding();
        match field {
            "pin" => {
                b.pins
                    .insert("/fixture/identity".into(), "not-a-sha256".into());
            }
            "path" => b.helper = "/fixture/../foreign".into(),
            "label" => b.label = "foreign/job".into(),
            "credential" => b.executable_path = "relative/path".into(),
            "uid" => b.uid = 0,
            _ => unreachable!(),
        }
        assert_eq!(
            code(SupervisionStore::prepare(&temp.0, &b, true)),
            "supervision_binding_invalid"
        );
    }
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 0);
}

#[test]
fn revision_overflow_is_refused_without_writing() {
    let temp = Temp::new();
    let store = temp.prepare();
    let before = fs::read(temp.state()).unwrap();
    let mut record = store.load().unwrap();
    record.revision = u64::MAX;
    assert_eq!(
        code(store.compare_and_set(u64::MAX, &record)),
        "supervision_revision_invalid"
    );
    assert_eq!(fs::read(temp.state()).unwrap(), before);
}

fn removed(store: &SupervisionStore) {
    let mut record = store.load().unwrap();
    record.revision += 1;
    record.registration = Registration::Removed;
    record.mode = Mode::Stopped;
    record.phase = Phase::Idle;
    record.code = Code::Stopped;
    assert!(store.compare_and_set(record.revision - 1, &record).unwrap());
}

#[test]
fn removed_store_purge_allows_fresh_install_without_touching_identity_or_journal() {
    let temp = Temp::new();
    let store = temp.prepare();
    let old_handle = temp.open();
    let mut record = enabled(&store);
    record.revision += 1;
    record.starts = 2;
    record.unclean = 1;
    assert!(store.compare_and_set(record.revision - 1, &record).unwrap());
    removed(&store);
    for name in [
        "configuration",
        "identity",
        "credential",
        "operation-journal",
    ] {
        fs::write(temp.0.join(name), b"preserve-existing-data").unwrap();
    }
    let owner = store.try_owner().unwrap();
    store.purge_removed(&owner).unwrap();
    assert!(!temp.0.join("supervision").exists());
    let fresh = temp.prepare();
    assert_eq!(fresh.load().unwrap(), Record::prepared());
    assert!(fresh.binding().unwrap() == binding());
    assert!(old_handle.load().is_err());
    record.revision += 1;
    assert!(old_handle
        .compare_and_set(record.revision - 1, &record)
        .is_err());
    assert_eq!(
        code(fresh.purge_removed(&owner)),
        "supervision_owner_mismatch"
    );
    for name in [
        "configuration",
        "identity",
        "credential",
        "operation-journal",
    ] {
        assert_eq!(
            fs::read(temp.0.join(name)).unwrap(),
            b"preserve-existing-data"
        );
    }
}

#[test]
fn purge_refuses_pending_corrupt_foreign_or_wrong_owner_without_deletion() {
    for variant in [
        "prepared", "unknown", "cleanup", "corrupt", "foreign", "owner", "artifact",
    ] {
        let temp = Temp::new();
        let mut b = binding();
        b.artifact = temp.0.join("service.plist");
        let store = SupervisionStore::prepare(&temp.0, &b, true).unwrap();
        let owner = store.try_owner().unwrap();
        match variant {
            "prepared" => {}
            "unknown" => {
                let mut r = store.load().unwrap();
                r.revision += 1;
                r.registration = Registration::Unknown;
                assert!(store.compare_and_set(r.revision - 1, &r).unwrap());
            }
            "cleanup" => {
                let mut r = installed(&store);
                r.revision += 1;
                r.phase = Phase::CleanupInFlight;
                assert!(store.compare_and_set(r.revision - 1, &r).unwrap());
            }
            _ => removed(&store),
        }
        if variant == "corrupt" {
            fs::write(temp.state(), b"corrupt").unwrap();
        }
        if variant == "foreign" {
            fs::write(temp.0.join("supervision/foreign"), b"do-not-delete").unwrap();
        }
        if variant == "artifact" {
            fs::write(&b.artifact, b"still-installed").unwrap();
        }
        let other = Temp::new();
        let other_store = other.prepare();
        let other_owner = other_store.try_owner().unwrap();
        let before = fs::read(temp.state()).unwrap();
        let guard = if variant == "owner" {
            &other_owner
        } else {
            &owner
        };
        assert!(store.purge_removed(guard).is_err(), "{variant}");
        assert_eq!(fs::read(temp.state()).unwrap(), before, "{variant}");
        assert!(temp.0.join("supervision/transaction.lock").exists());
        assert!(temp.0.join("supervision/owner.lock").exists());
        if variant == "foreign" {
            assert_eq!(
                fs::read(temp.0.join("supervision/foreign")).unwrap(),
                b"do-not-delete"
            );
        }
    }
}

#[test]
fn purge_and_rearm_cas_are_exclusive_and_late_handles_never_recreate_state() {
    let temp = Temp::new();
    let store = Arc::new(temp.prepare());
    removed(&store);
    let owner = store.try_owner().unwrap();
    let actor = temp.open();
    let mut next = actor.load().unwrap();
    next.revision += 1;
    next.arm += 1;
    next.registration = Registration::Installed;
    next.mode = Mode::Enabled;
    next.code = Code::Armed;
    let barrier = Arc::new(Barrier::new(2));
    let purge_barrier = barrier.clone();
    let purge_store = store.clone();
    let worker = std::thread::spawn(move || {
        purge_barrier.wait();
        purge_store.purge_removed(&owner)
    });
    barrier.wait();
    let cas = actor.compare_and_set(next.revision - 1, &next);
    let purge = worker.join().unwrap();
    assert!(!(matches!(cas, Ok(true)) && purge.is_ok()));
    if purge.is_ok() {
        assert!(!temp.0.join("supervision").exists());
        assert!(actor.load().is_err());
        assert!(actor.compare_and_set(next.revision - 1, &next).is_err());
        assert!(!temp.0.join("supervision").exists());
    } else {
        assert_eq!(actor.load().unwrap().mode, Mode::Enabled);
    }
}

#[test]
fn removed_purge_can_repeat_after_a_preflight_refusal_without_any_os_action() {
    let temp = Temp::new();
    let store = temp.prepare();
    removed(&store);
    let owner = store.try_owner().unwrap();
    let foreign = temp.0.join("supervision/fixture-extra");
    fs::write(&foreign, b"fixture-created-extra").unwrap();
    assert_eq!(
        code(store.purge_removed(&owner)),
        "supervision_purge_foreign_entry"
    );
    assert_eq!(store.load().unwrap().registration, Registration::Removed);
    // Remove only the extra fixture file we created; the store never does so.
    fs::remove_file(foreign).unwrap();
    store.purge_removed(&owner).unwrap();
    assert!(!temp.0.join("supervision").exists());
}

#[test]
fn owner_unlock_is_explicit_even_while_a_direct_owned_fork_inherits_the_fd() {
    use std::{
        fs::File,
        io::Read,
        os::fd::{AsRawFd, FromRawFd},
        time::{Duration, Instant},
    };
    struct Child {
        pid: libc::pid_t,
        release: Option<File>,
    }
    impl Child {
        fn alive(&mut self) -> bool {
            let result = unsafe { libc::waitpid(self.pid, std::ptr::null_mut(), libc::WNOHANG) };
            if result == self.pid {
                self.pid = 0;
                return false;
            }
            assert_eq!(result, 0);
            true
        }
    }
    impl Drop for Child {
        fn drop(&mut self) {
            // This is our unreaped fork child, never a PID from stored state.
            // Closing gives the child EOF without a write/SIGPIPE race.
            self.release.take();
            if self.pid == 0 {
                return;
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while self.alive() {
                if Instant::now() >= deadline {
                    unsafe {
                        libc::kill(self.pid, libc::SIGKILL);
                        libc::waitpid(self.pid, std::ptr::null_mut(), 0);
                    }
                    self.pid = 0;
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    let temp = Temp::new();
    let store = temp.prepare();
    let other = temp.open();
    let lease = store.try_owner().unwrap();
    let mut release = [0; 2];
    let mut ready = [0; 2];
    assert_eq!(unsafe { libc::pipe(release.as_mut_ptr()) }, 0);
    assert_eq!(unsafe { libc::pipe(ready.as_mut_ptr()) }, 0);
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        // Only async-signal-safe syscalls run after fork in the child.
        unsafe {
            libc::close(release[1]);
            libc::close(ready[0]);
            let byte = *b"R";
            libc::write(ready[1], byte.as_ptr().cast(), 1);
            libc::close(ready[1]);
            let mut release_byte = [0u8];
            libc::read(release[0], release_byte.as_mut_ptr().cast(), 1);
            libc::_exit(0);
        }
    }
    unsafe {
        libc::close(release[0]);
        libc::close(ready[1]);
    }
    let mut child = Child {
        pid,
        release: Some(unsafe { File::from_raw_fd(release[1]) }),
    };
    let mut reader = unsafe { File::from_raw_fd(ready[0]) };
    let mut poll = libc::pollfd {
        fd: reader.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    assert_eq!(unsafe { libc::poll(&mut poll, 1, 3000) }, 1);
    let mut byte = [0];
    reader.read_exact(&mut byte).unwrap();
    assert_eq!(byte, [b'R']);
    assert!(child.alive());
    assert_eq!(code(other.try_owner()), "supervision_owner_busy");
    drop(lease);
    let replacement = other.try_owner().unwrap();
    assert!(
        child.alive(),
        "unlock proof requires the inheriting child to still be alive"
    );
    drop(replacement);
}
