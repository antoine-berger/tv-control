use std::{
    fs,
    os::unix::fs::{symlink, MetadataExt, PermissionsExt},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tv_adapters::{pipeline_store::Store, runtime_intent::publish};

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
