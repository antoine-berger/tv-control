use super::*;

const RESOURCE_APP: &str = "1 activities found:\n  Activity #0:\n    ActivityInfo:\n      name=.Main\n      packageName=org.example.player\n      labelRes=0x7f030000 nonLocalizedLabel=null icon=0x0\n      ApplicationInfo:\n        packageName=org.example.player\n        sourceDir=/data/app/example/base.apk\n        enabled=true minSdkVersion=23 targetSdkVersion=34 versionCode=42\n";
const PROPS: &str = "[ro.serialno]: [device1]\n[ro.build.fingerprint]: [example/build]\n[persist.sys.locale]: [en-US]\n";
const TABLE: &[u8] = include_bytes!("../fixtures/android/aosp-basic.arsc");
fn resource_adapter(
    cache: &std::path::Path,
    metadata: &str,
    props: &str,
    extraction: bool,
) -> (GenericAdb, Arc<Mock>) {
    let process = Arc::new(Mock::new(&[
        VERSION,
        metadata,
        "No activities found",
        "No activities found",
        props,
    ]));
    if extraction {
        process
            .replies
            .lock()
            .unwrap()
            .push_back(Ok(TABLE.to_vec()));
    }
    (
        GenericAdb::new(
            Arc::new(cfg(false)),
            process.clone(),
            Arc::new(Probe(AtomicU32::new(41))),
        )
        .with_cache_dir(cache.to_path_buf()),
        process,
    )
}
#[test]
fn real_resource_labels_survive_restart_and_invalidate_on_version_or_locale() {
    let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
        "tv-label-cache-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let (adb, process) = resource_adapter(&path, RESOURCE_APP, PROPS, true);
    assert_eq!(
        adb.apps("screen", &ctx(&clock, &flag)).unwrap()[0]
            .label
            .as_deref(),
        Some("test1")
    );
    assert_eq!(process.calls.lock().unwrap().len(), 6);
    let (adb, process) = resource_adapter(&path, RESOURCE_APP, PROPS, false);
    assert_eq!(
        adb.apps("screen", &ctx(&clock, &flag)).unwrap()[0]
            .label
            .as_deref(),
        Some("test1")
    );
    assert_eq!(
        process.calls.lock().unwrap().len(),
        5,
        "restart must not extract unchanged resources"
    );
    for (metadata, properties) in [
        (
            RESOURCE_APP.replace("versionCode=42", "versionCode=43"),
            PROPS.to_owned(),
        ),
        (RESOURCE_APP.to_owned(), PROPS.replace("en-US", "fr-FR")),
        (RESOURCE_APP.to_owned(), PROPS.replace("device1", "device2")),
    ] {
        let (adb, process) = resource_adapter(&path, &metadata, &properties, true);
        assert_eq!(
            adb.apps("screen", &ctx(&clock, &flag)).unwrap()[0]
                .label
                .as_deref(),
            Some("test1")
        );
        assert_eq!(
            process.calls.lock().unwrap().len(),
            6,
            "changed identity needs fresh label extraction"
        );
    }
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn missing_resource_extractor_keeps_catalogue_but_not_invented_labels() {
    let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
        "tv-label-unavailable-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let (adb, process) = resource_adapter(&path, RESOURCE_APP, PROPS, false);
    process
        .replies
        .lock()
        .unwrap()
        .push_back(Err(Failure::before("adb_read_failed")));
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let apps = adb.apps("screen", &ctx(&clock, &flag)).unwrap();
    assert_eq!(apps[0].label, None);
    assert_eq!(apps[0].id, "org.example.player");
    assert!(!path.join("labels.json").exists());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn label_cache_refuses_symlink_ancestors_without_writing() {
    let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
        "tv-label-symlink-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let real = root.join("real");
    std::fs::create_dir(&real).unwrap();
    let alias = root.join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let cache = alias.join("cache");
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    // Both independent adapters must extract: no cache may be persisted through
    // an untrusted symlink, even though the resource label itself is valid.
    for _ in 0..2 {
        let (adb, process) = resource_adapter(&cache, RESOURCE_APP, PROPS, true);
        assert_eq!(
            adb.apps("screen", &ctx(&clock, &flag)).unwrap()[0]
                .label
                .as_deref(),
            Some("test1")
        );
        assert_eq!(process.calls.lock().unwrap().len(), 6);
        assert!(!real.join("cache").exists());
    }
    std::fs::remove_dir_all(root).unwrap();
}

// A one-pixel RGBA PNG fixture, produced with standard zlib and PNG chunk CRCs.
