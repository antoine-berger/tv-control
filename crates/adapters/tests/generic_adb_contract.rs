use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tv_adapters::{
    adb_server::ServerProbe,
    config::Config,
    generic_adb::GenericAdb,
    process::{Invocation, Output, Process},
    runtime::CancelFlag,
};
use tv_application::{generic::Device, Clock, Context};
use tv_domain::{generic::*, Failure, PortResult};

const VERSION: &str = "Android Debug Bridge version 1.0.41\n";
const FOREGROUND: &str = "mResumedActivity: ActivityRecord{aa u0 org.example.player/.Main t1}\n";
const UI: &str = "<hierarchy rotation=\"0\"><node text=\"Search\" class=\"android.widget.Button\" enabled=\"true\" clickable=\"true\" focused=\"true\" bounds=\"[10,20][110,60]\" /></hierarchy>";
const APP: &str = "1 activities found:\n  Activity #0:\n    ActivityInfo:\n      name=.Main\n      packageName=org.example.player\n      labelRes=0x0 nonLocalizedLabel=Example Player icon=0x0\n";
#[derive(Default)]
struct TestClock(AtomicU64);
impl Clock for TestClock {
    fn unix_ms(&self) -> u64 {
        1000 + self.monotonic_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
    fn sleep_ms(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::Relaxed);
    }
}
struct Mock {
    replies: Mutex<VecDeque<Result<Vec<u8>, Failure>>>,
    calls: Mutex<Vec<Invocation>>,
}
impl Mock {
    fn new(replies: &[&str]) -> Self {
        Self {
            replies: Mutex::new(replies.iter().map(|s| Ok(s.as_bytes().to_vec())).collect()),
            calls: Mutex::new(Vec::new()),
        }
    }
}
impl Process for Mock {
    fn run(&self, call: Invocation, ctx: &Context<'_>) -> PortResult<Output> {
        ctx.check()?;
        self.calls.lock().unwrap().push(call);
        Ok(Output {
            stdout: self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected subprocess")?,
            success: true,
        })
    }
}
struct Probe(AtomicU32);
impl ServerProbe for Probe {
    fn version(&self, _: &str, _: u16, ctx: &Context<'_>) -> PortResult<u32> {
        ctx.check()?;
        Ok(self.0.load(Ordering::Relaxed))
    }
}
fn cfg(granted: bool) -> Config {
    let mut cfg = Config::from_json(include_bytes!("../../../examples/config.json")).unwrap();
    cfg.generic_adb_control = granted;
    cfg
}
fn ctx<'a>(clock: &'a TestClock, cancel: &'a CancelFlag) -> Context<'a> {
    Context {
        clock,
        cancellation: cancel,
        deadline_monotonic_ms: clock.monotonic_ms() + 10_000,
    }
}
fn adapter(granted: bool, replies: &[&str]) -> (GenericAdb, Arc<Mock>, Arc<Probe>) {
    let process = Arc::new(Mock::new(replies));
    let probe = Arc::new(Probe(AtomicU32::new(41)));
    (
        GenericAdb::new(Arc::new(cfg(granted)), process.clone(), probe.clone()),
        process,
        probe,
    )
}
fn app() -> App {
    App {
        id: "org.example.player".into(),
        component: "org.example.player/.Main".into(),
        label: Some("Example Player".into()),
        search_component: Some("org.example.player/.Search".into()),
    }
}

#[test]
fn generic_grant_is_required_before_any_mutation() {
    let (adb, process, _) = adapter(false, &[]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let error = adb
        .apply(
            "screen",
            &Intent::OpenApp {
                app: "Example Player".into(),
            },
            Some(&app()),
            &ctx(&clock, &flag),
        )
        .unwrap_err();
    assert_eq!(error.code, "generic_control_not_authorized");
    assert!(process.calls.lock().unwrap().is_empty());
}
#[test]
fn version_cache_does_not_skip_live_protocol_guard() {
    let (adb, process, probe) = adapter(true, &[VERSION, FOREGROUND]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    adb.observe_for(
        "screen",
        &Intent::OpenApp {
            app: "Example Player".into(),
        },
        &ctx(&clock, &flag),
    )
    .unwrap();
    probe.0.store(40, Ordering::Relaxed);
    assert_eq!(
        adb.observe_for(
            "screen",
            &Intent::OpenApp {
                app: "Example Player".into()
            },
            &ctx(&clock, &flag)
        )
        .unwrap_err()
        .code,
        "adb_server_protocol_mismatch"
    );
    assert_eq!(process.calls.lock().unwrap().len(), 2);
}
#[test]
fn repeated_open_checks_do_not_read_unrelated_media_or_spawn_versions() {
    let (adb, process, _) = adapter(true, &[VERSION, FOREGROUND, FOREGROUND]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    for _ in 0..2 {
        let result = adb
            .observe_for(
                "screen",
                &Intent::OpenApp {
                    app: "Example Player".into(),
                },
                &ctx(&clock, &flag),
            )
            .unwrap();
        assert_eq!(result.active_app.as_deref(), Some("org.example.player"));
    }
    assert_eq!(process.calls.lock().unwrap().len(), 3);
}
#[test]
fn navigation_is_one_bounded_command() {
    let (adb, process, _) = adapter(true, &[VERSION, ""]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    adb.apply(
        "screen",
        &Intent::Navigate {
            keys: vec![Key::Down, Key::Down, Key::Select],
        },
        None,
        &ctx(&clock, &flag),
    )
    .unwrap();
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[1].args.last().unwrap(),
        "'input' 'keyevent' '20' '20' '23'"
    );
    assert!(calls[1].mutating);
}
#[test]
fn search_query_is_one_quoted_argument_and_requires_handler() {
    let (adb, process, _) = adapter(true, &[VERSION, "Status: ok"]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let query = "it's fun; reboot $(x)";
    adb.apply(
        "screen",
        &Intent::Search {
            app: "Example Player".into(),
            query: query.into(),
        },
        Some(&app()),
        &ctx(&clock, &flag),
    )
    .unwrap();
    let calls = process.calls.lock().unwrap();
    let command = calls[1].args.last().unwrap().to_str().unwrap();
    assert!(command.ends_with("'it'\\''s fun; reboot $(x)'"));
    drop(calls);
    let mut unsupported = app();
    unsupported.search_component = None;
    assert_eq!(
        adb.apply(
            "screen",
            &Intent::Search {
                app: "Example Player".into(),
                query: "q".into()
            },
            Some(&unsupported),
            &ctx(&clock, &flag)
        )
        .unwrap_err()
        .code,
        "search_unsupported"
    );
    assert_eq!(process.calls.lock().unwrap().len(), 2);
}
#[test]
fn discovered_apps_are_current_user_components_without_configuration_aliases() {
    let (adb, process, _) = adapter(
        false,
        &[VERSION, APP, "No activities found", "No activities found"],
    );
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let apps = adb.apps("screen", &ctx(&clock, &flag)).unwrap();
    assert_eq!(apps[0].label.as_deref(), Some("Example Player"));
    assert_eq!(apps[0].id, "org.example.player");
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert!(calls[1..].iter().all(|c| !c.mutating
        && c.args
            .last()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("'--user' 'current'")));
}
#[test]
fn stale_ui_ref_cannot_tap() {
    let changed = UI.replace("Search", "Purchase");
    let (adb, process, _) = adapter(true, &[VERSION, UI, &changed]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    assert_eq!(
        adb.apply(
            "screen",
            &Intent::Activate {
                reference: view.elements[0].reference.clone()
            },
            None,
            &ctx(&clock, &flag)
        )
        .unwrap_err()
        .code,
        "ui_reference_stale"
    );
    assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
}
#[test]
fn fresh_ui_ref_is_consumed_and_expiry_cannot_dispatch() {
    let (adb, process, _) = adapter(true, &[VERSION, UI, UI, ""]);
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    let intent = Intent::Activate {
        reference: view.elements[0].reference.clone(),
    };
    adb.apply("screen", &intent, None, &ctx(&clock, &flag))
        .unwrap();
    assert_eq!(
        adb.apply("screen", &intent, None, &ctx(&clock, &flag))
            .unwrap_err()
            .code,
        "ui_reference_unknown"
    );
    assert_eq!(
        process
            .calls
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .args
            .last()
            .unwrap(),
        "'input' 'tap' '60' '40'"
    );
    let (adb, process, _) = adapter(true, &[VERSION, UI]);
    let view = adb.ui("screen", &ctx(&clock, &flag)).unwrap();
    clock.sleep_ms(30_001);
    assert_eq!(
        adb.apply(
            "screen",
            &Intent::Activate {
                reference: view.elements[0].reference.clone()
            },
            None,
            &ctx(&clock, &flag)
        )
        .unwrap_err()
        .code,
        "ui_reference_expired"
    );
    assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
}
#[test]
fn duplicate_media_sessions_cannot_dispatch() {
    let (adb, process, _) = adapter(
        true,
        &[
            VERSION,
            FOREGROUND,
            "tag=x, package=org.example.player\ntag=x, package=org.other.app",
        ],
    );
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    assert_eq!(
        adb.apply("screen", &Intent::Pause, None, &ctx(&clock, &flag))
            .unwrap_err()
            .code,
        "media_tag_ambiguous"
    );
    assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
}

const RESOURCE_APP: &str = "1 activities found:\n  Activity #0:\n    ActivityInfo:\n      name=.Main\n      packageName=org.example.player\n      labelRes=0x7f030000 nonLocalizedLabel=null icon=0x0\n      ApplicationInfo:\n        packageName=org.example.player\n        sourceDir=/data/app/example/base.apk\n        enabled=true minSdkVersion=23 targetSdkVersion=34 versionCode=42\n";
const PROPS: &str = "[ro.serialno]: [device1]\n[ro.build.fingerprint]: [example/build]\n[persist.sys.locale]: [en-US]\n";
const TABLE: &[u8] = include_bytes!("fixtures/android/aosp-basic.arsc");
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
    let path = std::env::temp_dir().join(format!(
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
    let path = std::env::temp_dir().join(format!(
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

// A one-pixel RGBA PNG fixture, produced with standard zlib and PNG chunk CRCs.
const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 96, 96, 96, 248, 15, 0, 1,
    4, 1, 0, 95, 229, 195, 75, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
#[test]
fn screenshots_are_explicit_bounded_and_binary_without_pty() {
    let (adb, process, _) = adapter(false, &[VERSION]);
    process.replies.lock().unwrap().push_back(Ok(PNG.to_vec()));
    let clock = TestClock::default();
    let flag = CancelFlag::default();
    let result = adb.screenshot("screen", &ctx(&clock, &flag)).unwrap();
    assert_eq!((result.width, result.height), (1, 1));
    assert_eq!(result.png, PNG);
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls[1].args[calls[1].args.len() - 2], "exec-out");
    assert_eq!(calls[1].args.last().unwrap(), "'screencap' '-p'");
    assert!(!calls[1].mutating);
}
#[test]
fn screenshots_reject_missing_oversized_and_corrupt_png() {
    let mut corrupt = PNG.to_vec();
    corrupt[20] ^= 1;
    for bad in [
        Vec::new(),
        vec![0; 1024 * 1024 + 1],
        b"protected screen".to_vec(),
        corrupt,
        PNG[..PNG.len() - 12].to_vec(),
    ] {
        let (adb, process, _) = adapter(false, &[VERSION]);
        process.replies.lock().unwrap().push_back(Ok(bad));
        let clock = TestClock::default();
        let flag = CancelFlag::default();
        assert!(adb.screenshot("screen", &ctx(&clock, &flag)).is_err());
    }
}
