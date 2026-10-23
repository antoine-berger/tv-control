use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
use tv_adapters::pipeline_store::*;

struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "tv-pipeline-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn state(&self) -> Checkpoint {
        Checkpoint {
            schema: 1,
            installation_id: "fixture-installation".into(),
            configuration_path: self.0.join("configuration.json"),
            configuration_revision: "fixture-revision".into(),
            device: "screen".into(),
            stage: ProgressStage::Configuration,
            outcome: ProgressOutcome::Pending,
            tunnel_id: None,
            tunnel_alias: None,
        }
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn private_atomic_choices_resume_without_authorization_claims() {
    let temp = Temporary::new();
    let directory = temp.0.join("progress");
    let mut store = Store::open(&directory).unwrap();
    assert_eq!(store.load().unwrap(), None);
    let mut state = temp.state();
    store.save(&state).unwrap();
    fs::write(
        directory.join(".pipeline-progress.json-0-99.tmp"),
        b"{partial",
    )
    .unwrap();
    state.stage = ProgressStage::ExternalConversation;
    state.outcome = ProgressOutcome::Pending;
    store.save(&state).unwrap();
    drop(store);
    let store = Store::open(&directory).unwrap();
    assert_eq!(store.load().unwrap(), Some(state));
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(directory.join("progress.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("progress.json")).unwrap()).unwrap();
    for forbidden in [
        "credentials",
        "authorized",
        "nonce",
        "receipt",
        "runtime_key",
    ] {
        assert!(value.get(forbidden).is_none());
    }
}
#[test]
fn typed_schema_rejects_credentials_and_corrupt_data_without_overwriting() {
    let temp = Temporary::new();
    let directory = temp.0.join("progress");
    let mut store = Store::open(&directory).unwrap();
    store.save(&temp.state()).unwrap();
    drop(store);
    let path = directory.join("progress.json");
    for bytes in [b"{partial".to_vec(), {
        let mut value = serde_json::to_value(temp.state()).unwrap();
        value["credentials"] = serde_json::json!("fixture-never-log");
        serde_json::to_vec(&value).unwrap()
    }] {
        fs::write(&path, &bytes).unwrap();
        let mut store = Store::open(&directory).unwrap();
        assert_eq!(store.load(), Err("pipeline_checkpoint_corrupt"));
        assert_eq!(
            store.save(&temp.state()),
            Err("pipeline_checkpoint_corrupt")
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        drop(store);
    }
}
#[test]
fn symlinks_fifo_and_unrecognized_links_fail_without_modifying_targets() {
    let temp = Temporary::new();
    let directory = temp.0.join("progress");
    drop(Store::open(&directory).unwrap());
    let path = directory.join("progress.json");
    let target = temp.0.join("keep");
    fs::write(&target, b"keep").unwrap();
    symlink(&target, &path).unwrap();
    assert!(Store::open(&directory).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    fs::remove_file(&path).unwrap();
    let path_c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path_c.as_ptr(), 0o600) }, 0);
    assert!(Store::open(&directory).is_err());
    fs::remove_file(&path).unwrap();
    let mut store = Store::open(&directory).unwrap();
    store.save(&temp.state()).unwrap();
    drop(store);
    fs::hard_link(&path, directory.join("not-ours")).unwrap();
    assert!(matches!(
        Store::open(&directory),
        Err("pipeline_unrecognized_links")
    ));
    assert!(directory.join("not-ours").exists());
}
#[test]
fn changed_and_replaced_files_are_preserved() {
    for replace in [false, true] {
        let temp = Temporary::new();
        let directory = temp.0.join("progress");
        let path = directory.join("progress.json");
        let mut store = Store::open(&directory).unwrap();
        store.save(&temp.state()).unwrap();
        if replace {
            fs::remove_file(&path).unwrap();
        }
        fs::write(&path, b"external edit").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(store.save(&temp.state()), Err("pipeline_storage_conflict"));
        assert_eq!(fs::read(&path).unwrap(), b"external edit");
    }
}
#[test]
fn logs_are_bounded_typed_private_and_detect_external_changes() {
    let temp = Temporary::new();
    let directory = temp.0.join("progress");
    drop(Store::open(&directory).unwrap());
    let path = directory.join("events.jsonl");
    let mut seed = Vec::new();
    let mut count = 0;
    loop {
        let mut line = serde_json::to_vec(&LogEvent {
            stage: ProgressStage::LocalMcp,
            code: EventCode::Checking,
            elapsed_ms: count,
        })
        .unwrap();
        line.push(b'\n');
        if seed.len() + line.len() > MAX_LOG_BYTES {
            break;
        }
        seed.extend(line);
        count += 1;
    }
    fs::write(&path, seed).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut store = Store::open(&directory).unwrap();
    for elapsed_ms in count..count + 20 {
        store
            .event(LogEvent {
                stage: ProgressStage::LocalMcp,
                code: EventCode::Checking,
                elapsed_ms,
            })
            .unwrap();
    }
    let path = store.log_path();
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.len() <= MAX_LOG_BYTES);
    let events: Vec<LogEvent> = bytes
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    assert_eq!(events.last().unwrap().elapsed_ms, count + 19);
    assert!(events[0].elapsed_ms > 0);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::write(&path, b"external log edit").unwrap();
    assert_eq!(
        store.event(LogEvent {
            stage: ProgressStage::LocalMcp,
            code: EventCode::Checking,
            elapsed_ms: 1000
        }),
        Err("pipeline_storage_conflict")
    );
    assert_eq!(fs::read(&path).unwrap(), b"external log edit");
}
#[test]
fn interrupted_first_publication_recovers_only_its_exact_temporary_link() {
    let temp = Temporary::new();
    let directory = temp.0.join("progress");
    let mut store = Store::open(&directory).unwrap();
    store.save(&temp.state()).unwrap();
    drop(store);
    let path = directory.join("progress.json");
    let temporary = directory.join(".pipeline-progress.json-12-34.tmp");
    fs::hard_link(&path, &temporary).unwrap();
    let store = Store::open(&directory).unwrap();
    assert_eq!(store.load().unwrap(), Some(temp.state()));
    assert!(!temporary.exists());
}
#[test]
fn session_lock_is_exclusive_and_explicitly_released() {
    let temp = Temporary::new();
    let directory = temp.0.join("progress");
    let store = Store::open(&directory).unwrap();
    assert!(matches!(Store::open(&directory), Err("pipeline_busy")));
    drop(store);
    assert!(Store::open(&directory).is_ok());
}
#[test]
fn dropping_the_owner_unlocks_a_descriptor_temporarily_inherited_by_fork() {
    let temp = Temporary::new();
    let directory = temp.0.join("progress");
    let store = Store::open(&directory).unwrap();
    let mut pipe = [0; 2];
    assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        // No Rust allocation or destructor between fork and _exit in a threaded process.
        unsafe {
            libc::close(pipe[1]);
            let mut byte = 0u8;
            libc::read(pipe[0], &mut byte as *mut _ as *mut libc::c_void, 1);
            libc::_exit(0);
        }
    }
    unsafe {
        libc::close(pipe[0]);
    }
    drop(store);
    let acquired = Store::open(&directory);
    unsafe {
        let byte = 1u8;
        libc::write(pipe[1], &byte as *const _ as *const libc::c_void, 1);
        libc::close(pipe[1]);
        libc::waitpid(pid, std::ptr::null_mut(), 0);
    }
    assert!(acquired.is_ok());
}
#[test]
fn corrupt_checkpoint_can_be_preserved_for_explicit_guided_recovery() {
    let temp = Temporary::new();
    let directory = temp.0.join("progress");
    drop(Store::open(&directory).unwrap());
    let path = directory.join("progress.json");
    fs::write(&path, b"{partial").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut store = Store::open(&directory).unwrap();
    assert!(store.load().is_err());
    let backup = store.quarantine_checkpoint().unwrap();
    assert_eq!(fs::read(backup).unwrap(), b"{partial");
    assert_eq!(store.load().unwrap(), None);
    store.save(&temp.state()).unwrap();
    assert_eq!(store.load().unwrap(), Some(temp.state()));
}
#[test]
fn pipeline_crash_child() {
    let Ok(path) = std::env::var("TV_PIPELINE_STORE_CRASH_FIXTURE") else {
        return;
    };
    let directory = PathBuf::from(path);
    let mut store = Store::open(&directory).unwrap();
    let state = Checkpoint {
        schema: 1,
        installation_id: "fixture-crash".into(),
        configuration_path: directory.join("config.json"),
        configuration_revision: "revision".into(),
        device: "screen".into(),
        stage: ProgressStage::ExternalConversation,
        outcome: ProgressOutcome::Pending,
        tunnel_id: None,
        tunnel_alias: None,
    };
    store.save(&state).unwrap();
    std::process::exit(0);
}
#[test]
fn process_exit_releases_lock_and_keeps_resumable_progress() {
    let temp = Temporary::new();
    let directory = temp.0.join("progress");
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "pipeline_crash_child"])
        .env("TV_PIPELINE_STORE_CRASH_FIXTURE", &directory)
        .status()
        .unwrap();
    assert!(status.success());
    let store = Store::open(&directory).unwrap();
    assert_eq!(
        store.load().unwrap().unwrap().stage,
        ProgressStage::ExternalConversation
    );
}
