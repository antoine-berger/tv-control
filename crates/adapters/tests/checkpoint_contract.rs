use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
use tv_adapters::setup_checkpoint::{Checkpoint, Store};
struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "tv-checkpoint-{}-{}",
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
            writer_version: "0.2.3".into(),
            output: self.0.join("config.json"),
            interface: Some("lan-fixture@192.0.2.1".into()),
            ..Checkpoint::default()
        }
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn atomic_private_state_updates_ignore_incomplete_temporary_files() {
    let temp = Temporary::new();
    let path = temp.0.join("state.json");
    let mut store = Store::open(&path).unwrap();
    assert!(store.load().unwrap().is_none());
    let mut state = temp.state();
    store.save(&state).unwrap();
    fs::write(temp.0.join(".tv-setup-interrupted.tmp"), b"{").unwrap();
    state.device = Some("screen_192_0_2_10".into());
    store.save(&state).unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert_eq!(loaded.device, state.device);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert!(value.get("credentials").is_none());
    assert!(value.get("authorized").is_none());
}
#[test]
fn corruption_symlinks_and_changed_files_are_preserved() {
    let temp = Temporary::new();
    let path = temp.0.join("state.json");
    fs::write(&path, b"{partial").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.load().unwrap_err(), "checkpoint_corrupt_use_new_path");
    drop(store);
    assert_eq!(fs::read(&path).unwrap(), b"{partial");
    fs::remove_file(&path).unwrap();
    let target = temp.0.join("personal-data");
    fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let mut store = Store::open(&path).unwrap();
    assert!(store.load().is_err());
    drop(store);
    assert_eq!(fs::read(target).unwrap(), b"keep");
    fs::remove_file(&path).unwrap();
    let mut store = Store::open(&path).unwrap();
    assert!(store.load().unwrap().is_none());
    store.save(&temp.state()).unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"replacement").unwrap();
    assert_eq!(store.save(&temp.state()), Err("checkpoint_conflict"));
    assert_eq!(fs::read(path).unwrap(), b"replacement");
}
#[test]
fn private_schema_refuses_credentials_and_incompatible_versions() {
    let temp = Temporary::new();
    let mut value = serde_json::to_value(temp.state()).unwrap();
    value["credentials"] = serde_json::json!("not-a-secret-fixture");
    assert!(serde_json::from_value::<Checkpoint>(value).is_err());
    let mut state = temp.state();
    state.schema = 2;
    assert_eq!(state.validate(), Err("checkpoint_invalid"));
    let path = temp.0.join("state.json");
    let mut store = Store::open(&path).unwrap();
    store.save(&temp.state()).unwrap();
    drop(store);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        Store::open(&path).unwrap().load().unwrap_err(),
        "checkpoint_not_private"
    );
}
#[test]
fn checkpoint_crash_child() {
    let Some(path) = std::env::var_os("TV_CHECKPOINT_CHILD") else {
        return;
    };
    let path = PathBuf::from(path);
    let mut store = Store::open(&path).unwrap();
    let state = Checkpoint {
        schema: 1,
        writer_version: "0.2.3".into(),
        output: path.parent().unwrap().join("config.json"),
        interface: Some("lan-fixture@192.0.2.1".into()),
        ..Checkpoint::default()
    };
    store.save(&state).unwrap();
    std::process::exit(17);
}
#[test]
fn process_exit_releases_lock_and_retains_completed_checkpoint() {
    let temp = Temporary::new();
    let path = temp.0.join("state.json");
    let code = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "checkpoint_crash_child"])
        .env("TV_CHECKPOINT_CHILD", &path)
        .status()
        .unwrap();
    assert_eq!(code.code(), Some(17));
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store.load().unwrap().unwrap().interface,
        Some("lan-fixture@192.0.2.1".into())
    );
    assert!(Store::open(&path).is_err());
}
#[test]
fn discovery_io_diagnostic_retains_numeric_evidence_without_raw_messages() {
    let failure = tv_adapters::discovery::io_failure(
        "mdns_send_failed",
        std::io::Error::from_raw_os_error(13),
    );
    assert_eq!(failure.os_error, Some(13));
    assert_eq!(failure.code, "mdns_send_failed");
    assert_eq!(failure.effect, tv_domain::Effect::None);
}
