use std::{
    fs,
    os::unix::fs::{symlink, MetadataExt, PermissionsExt},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tv_adapters::{
    pipeline_store::Store,
    runtime_intent::{publish, replace_after_stop},
};

struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "runtime intent's {}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn large_intent_publishes_privately_and_identical_resume_does_not_replace_it() {
    let temp = Temporary::new();
    let directory = temp.0.join("state");
    let _session = Store::open(&directory).unwrap();
    let bytes = vec![b'x'; 1_048_577];
    publish(&directory, &bytes).unwrap();
    let path = directory.join("runtime-intent.json");
    let before = fs::metadata(&path).unwrap();
    assert_eq!(before.mode() & 0o777, 0o600);
    assert_eq!(before.nlink(), 1);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    publish(&directory, &bytes).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().ino(), before.ino());
    assert_eq!(
        publish(&directory, b"other"),
        Err("runtime_intent_conflict")
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(
        publish(&directory, &vec![b'x'; 6_400_001]),
        Err("runtime_intent_too_large")
    );
    assert_eq!(fs::metadata(&path).unwrap().ino(), before.ino());
}

#[test]
fn symlinks_and_nonprivate_directories_are_refused_without_touching_the_target() {
    let temp = Temporary::new();
    let directory = temp.0.join("state");
    let _session = Store::open(&directory).unwrap();
    let target = temp.0.join("other");
    fs::write(&target, b"preserve").unwrap();
    symlink(&target, directory.join("runtime-intent.json")).unwrap();
    assert!(publish(&directory, b"new").is_err());
    assert_eq!(fs::read(target).unwrap(), b"preserve");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        publish(&directory, b"new"),
        Err("runtime_intent_directory_not_private")
    );
}

#[test]
fn recognized_initial_publication_link_is_recovered_and_partial_data_is_preserved() {
    for (existing, expected) in [
        (b"complete".as_slice(), Ok(())),
        (b"{partial".as_slice(), Err("runtime_intent_conflict")),
    ] {
        let temp = Temporary::new();
        let directory = temp.0.join("state");
        let _session = Store::open(&directory).unwrap();
        let staged = directory.join(".pipeline-runtime-intent.json-123-456.tmp");
        fs::write(&staged, existing).unwrap();
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).unwrap();
        let path = directory.join("runtime-intent.json");
        fs::hard_link(&staged, &path).unwrap();
        assert_eq!(publish(&directory, b"complete"), expected);
        assert!(!staged.exists());
        assert_eq!(fs::read(&path).unwrap(), existing);
        assert_eq!(fs::metadata(&path).unwrap().nlink(), 1);
    }
}

#[test]
fn unrecognized_links_and_unpublished_partial_files_are_preserved() {
    let temp = Temporary::new();
    let directory = temp.0.join("state");
    let _session = Store::open(&directory).unwrap();
    let path = directory.join("runtime-intent.json");
    fs::write(&path, b"preserve").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let unknown = directory.join("unknown-link");
    fs::hard_link(&path, &unknown).unwrap();
    assert_eq!(
        publish(&directory, b"preserve"),
        Err("pipeline_unrecognized_links")
    );
    assert!(unknown.exists());
    assert_eq!(fs::read(&path).unwrap(), b"preserve");
    fs::remove_file(&unknown).unwrap();
    let orphan = directory.join(".pipeline-runtime-intent.json-123-999.tmp");
    fs::write(&orphan, b"{partial").unwrap();
    assert_eq!(
        publish(&directory, b"different"),
        Err("runtime_intent_conflict")
    );
    assert_eq!(fs::read(orphan).unwrap(), b"{partial");
}

#[test]
fn approved_stopped_replacement_preserves_old_intent_and_publishes_new_privately() {
    let temp = Temporary::new();
    let directory = temp.0.join("state");
    let _session = Store::open(&directory).unwrap();
    publish(&directory, b"old approved configuration").unwrap();
    let backup = replace_after_stop(
        &directory,
        b"old approved configuration",
        b"new approved configuration",
    )
    .unwrap();
    assert_eq!(fs::read(&backup).unwrap(), b"old approved configuration");
    assert_eq!(fs::metadata(&backup).unwrap().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(&backup).unwrap().nlink(), 1);
    let current = directory.join("runtime-intent.json");
    assert_eq!(fs::read(&current).unwrap(), b"new approved configuration");
    assert_eq!(fs::metadata(current).unwrap().mode() & 0o777, 0o600);
    publish(&directory, b"new approved configuration").unwrap();
    assert_eq!(
        replace_after_stop(&directory, b"old approved configuration", b"third"),
        Err("runtime_intent_conflict")
    );
    assert_eq!(fs::read(backup).unwrap(), b"old approved configuration");
}

#[test]
fn replacement_never_creates_a_missing_old_intent_or_follows_a_symlink() {
    let temp = Temporary::new();
    let directory = temp.0.join("state");
    let _session = Store::open(&directory).unwrap();
    let path = directory.join("runtime-intent.json");
    assert_eq!(
        replace_after_stop(&directory, b"old", b"new"),
        Err("runtime_intent_conflict")
    );
    assert!(!path.exists());
    let target = temp.0.join("keep");
    fs::write(&target, b"preserve").unwrap();
    symlink(&target, &path).unwrap();
    assert!(replace_after_stop(&directory, b"preserve", b"new").is_err());
    assert_eq!(fs::read(target).unwrap(), b"preserve");
    assert_eq!(fs::read_dir(directory).unwrap().count(), 2);
}

#[test]
fn every_bridge_start_gets_a_new_generation_bound_to_its_intent() {
    use tv_adapters::runtime_intent::{read_generation, start_generation};
    let temp = Temporary::new();
    let directory = temp.0.join("state");
    let _session = Store::open(&directory).unwrap();
    assert_eq!(
        read_generation(&directory, "installation", "revision", "screen"),
        Err("bridge_generation_unavailable")
    );
    let first = start_generation(&directory, "installation", "revision", "screen").unwrap();
    assert_eq!(
        read_generation(&directory, "installation", "revision", "screen").unwrap(),
        first.generation
    );
    let second = start_generation(&directory, "installation", "revision", "screen").unwrap();
    assert_ne!(first.generation, second.generation);
    let staged = directory.join(".pipeline-bridge-generation.json-123-456.tmp");
    fs::hard_link(directory.join("bridge-generation.json"), &staged).unwrap();
    assert_eq!(
        read_generation(&directory, "installation", "revision", "screen"),
        Err("bridge_generation_pending")
    );
    assert!(
        staged.exists(),
        "a concurrent reader must not recover the publisher's live link"
    );
    fs::remove_file(staged).unwrap();
    assert_eq!(
        read_generation(&directory, "installation", "old_revision", "screen"),
        Err("bridge_generation_binding_mismatch")
    );
    assert_eq!(
        start_generation(&directory, "foreign_installation", "revision", "screen").err(),
        Some("bridge_generation_installation_mismatch")
    );
    assert_eq!(
        read_generation(&directory, "installation", "revision", "screen").unwrap(),
        second.generation
    );
    let path = directory.join("bridge-generation.json");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    fs::remove_file(&path).unwrap();
    let target = temp.0.join("keep_generation");
    fs::write(&target, b"preserve").unwrap();
    symlink(&target, path).unwrap();
    assert!(start_generation(&directory, "installation", "revision", "screen").is_err());
    assert_eq!(fs::read(target).unwrap(), b"preserve");
}
