use super::*;
use crate::{
    adb_server::ServerProbe,
    config::{Config, Device},
    process::{Invocation, Output, Process},
    runtime::CancelFlag,
};
use std::{
    collections::VecDeque,
    ffi::OsString,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tv_application::{Cancellation, Clock};
use tv_domain::Effect;

const VERSION: &str = "Android Debug Bridge version 1.0.41\n";
const COMPONENT: &str = "org.example.alpha/.Main\n";

#[derive(Default)]
struct FakeClock(AtomicU64);
impl Clock for FakeClock {
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

struct MockProcess {
    replies: Mutex<VecDeque<Output>>,
    calls: Mutex<Vec<Invocation>>,
    cancel_after: Option<(usize, Arc<CancelFlag>)>,
    expire_after: Option<(usize, Arc<FakeClock>)>,
}
impl MockProcess {
    fn new(output: &str) -> Self {
        Self {
            replies: Mutex::new(
                [VERSION, output]
                    .into_iter()
                    .map(|text| Output {
                        stdout: text.as_bytes().to_vec(),
                        success: true,
                    })
                    .collect(),
            ),
            calls: Mutex::new(Vec::new()),
            cancel_after: None,
            expire_after: None,
        }
    }
}
impl Process for MockProcess {
    fn run(&self, invocation: Invocation, _: &Context<'_>) -> PortResult<Output> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(invocation);
        if let Some((count, flag)) = &self.cancel_after {
            if calls.len() == *count {
                flag.cancel();
            }
        }
        if let Some((count, clock)) = &self.expire_after {
            if calls.len() == *count {
                clock.0.store(100, Ordering::SeqCst);
            }
        }
        Ok(self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected subprocess"))
    }
}

struct MockProbe {
    calls: AtomicUsize,
    version: u32,
    cancel: Option<Arc<CancelFlag>>,
}
impl Default for MockProbe {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            version: 41,
            cancel: None,
        }
    }
}
impl ServerProbe for MockProbe {
    fn version(&self, host: &str, port: u16, _: &Context<'_>) -> PortResult<u32> {
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 15037);
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(flag) = &self.cancel {
            flag.cancel();
        }
        Ok(self.version)
    }
}

fn config() -> Config {
    Config::from_json(include_bytes!("../../../../../examples/config.json")).unwrap()
}
fn context<'a>(clock: &'a dyn Clock, cancellation: &'a dyn Cancellation) -> Context<'a> {
    Context {
        clock,
        cancellation,
        deadline_monotonic_ms: 100,
    }
}
fn parse(output: &str) -> PortResult<Vec<String>> {
    parse_apps(
        output,
        MAX_OUTPUT_BYTES,
        &context(&FakeClock::default(), &CancelFlag::default()),
    )
}
fn failure(result: PortResult<Vec<String>>, code: &'static str) {
    assert_eq!(result.unwrap_err(), Failure::before(code));
}

#[test]
fn discovers_generic_sorted_unique_packages_without_changing_configuration() {
    let mut config = config();
    config.apps.clear();
    let before = serde_json::to_vec(&config).unwrap();
    let process = MockProcess::new(
        "org.example.zeta/.Main\r\norg.example.alpha/com.generic.Player$Entry\r\norg.example.zeta/.Other\r\norg.example.alpha/com.generic.Player$Entry\r\n",
    );
    let probe = MockProbe::default();
    let found = Adb::new(&config, &process, &probe)
        .discover_apps(
            "screen",
            &context(&FakeClock::default(), &CancelFlag::default()),
        )
        .unwrap();
    assert_eq!(found, ["org.example.alpha", "org.example.zeta"]);
    assert_eq!(serde_json::to_vec(&config).unwrap(), before);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].args, [OsString::from("version")]);
    let Device::Adb { endpoint, .. } = &config.devices["screen"] else {
        panic!("expected ADB fixture")
    };
    let expected: Vec<OsString> = [
        "-H", "127.0.0.1", "-P", "15037", "-s", endpoint, "shell",
        "'cmd' 'package' 'query-activities' '--components' '--user' 'current' '-a' 'android.intent.action.MAIN' '-c' 'android.intent.category.LEANBACK_LAUNCHER'",
    ].into_iter().map(Into::into).collect();
    assert_eq!(calls[1].args, expected);
    assert_eq!(
        calls[1].env.get(&OsString::from("ADB_MDNS")),
        Some(&OsString::from("0"))
    );
    assert_eq!(
        calls[1].env.get(&OsString::from("ADB_USB")),
        Some(&OsString::from("0"))
    );
    assert!(calls
        .iter()
        .all(|call| !call.mutating && call.stdin.is_empty()));
}

#[test]
fn explicit_no_launchers_is_empty_but_missing_output_is_not() {
    for output in [
        "No activities found",
        "No activities found\n",
        "No activities found\r\n",
    ] {
        assert!(parse(output).unwrap().is_empty());
    }
    for output in ["", "\n", "\r\n", " ", "No activities found\n\n"] {
        failure(parse(output), "app_discovery_output_invalid");
    }
}

#[test]
fn rejects_hostile_malformed_or_mixed_output_without_partial_results() {
    for output in [
        "org.example.alpha/.Main;reboot\n",
        "org.example.alpha/.Main$(reboot)\n",
        "org.example.alpha/.Main`reboot`\n",
        "org.example.alpha/.Main'\n",
        "org.example.alpha/.Main\0\n",
        "org.example.alpha/.Main\x1b[0m\n",
        "org.example.alpha/.Main\t\n",
        "org.example.alpha/.Main/Other\n",
        "org.example.alpha/.Main Other\n",
        "org.example.alpha/.Main\rOther\n",
        "org.example.alpha/..Main\n",
        "org.example.alpha/.1Main\n",
        "org.example.alpha/\n",
        "org.example.alpha/.\n",
        "org.example.alpha\n",
        "/.Main\n",
        "org..alpha/.Main\n",
        "org.1alpha/.Main\n",
        "org.example.alpha-evil/.Main\n",
        "org.exámple.alpha/.Main\n",
        "org.example.alpha/.Máin\n",
        " org.example.alpha/.Main\n",
        "package:org.example.alpha\n",
        "1 activities found:\norg.example.alpha/.Main\n",
        "Error: package manager unavailable\n",
        "No activities found\norg.example.alpha/.Main\n",
        "org.example.alpha/.Main\nNo activities found\n",
        "No activities found\nNo activities found\n",
        "org.example.alpha/.Main\nmalformed\n",
    ] {
        failure(parse(output), "app_discovery_output_invalid");
    }
}

#[test]
fn enforces_byte_component_row_and_package_caps_without_truncation() {
    failure(
        parse(&"x".repeat(MAX_OUTPUT_BYTES + 1)),
        "app_discovery_output_limit",
    );
    let clock = FakeClock::default();
    let flag = CancelFlag::default();
    failure(
        parse_apps(COMPONENT, COMPONENT.len() - 1, &context(&clock, &flag)),
        "app_discovery_output_limit",
    );
    assert_eq!(
        parse_apps(COMPONENT, COMPONENT.len(), &context(&clock, &flag))
            .unwrap()
            .len(),
        1
    );
    let long = format!("org.example.alpha/.{}\n", "M".repeat(MAX_COMPONENT_BYTES));
    failure(parse(&long), "app_discovery_output_limit");
    assert_eq!(parse(&COMPONENT.repeat(MAX_COMPONENTS)).unwrap().len(), 1);
    failure(
        parse(&COMPONENT.repeat(MAX_COMPONENTS + 1)),
        "app_discovery_output_limit",
    );
    let mut output = (0..MAX_PACKAGES)
        .map(|i| format!("org.example.app{i}/.Main\n"))
        .collect::<String>();
    assert_eq!(parse(&output).unwrap().len(), MAX_PACKAGES);
    output.push_str("org.example.extra/.Main\n");
    failure(parse(&output), "app_discovery_output_limit");
}

#[test]
fn cancelled_or_expired_requests_do_not_start_preflight() {
    let config = config();
    let process = MockProcess::new(COMPONENT);
    let probe = MockProbe::default();
    let clock = FakeClock::default();
    let flag = CancelFlag::default();
    flag.cancel();
    failure(
        Adb::new(&config, &process, &probe).discover_apps("screen", &context(&clock, &flag)),
        "cancelled",
    );
    clock.0.store(100, Ordering::SeqCst);
    failure(
        Adb::new(&config, &process, &probe)
            .discover_apps("screen", &context(&clock, &CancelFlag::default())),
        "expired",
    );
    assert!(process.calls.lock().unwrap().is_empty());
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn cancellation_at_either_preflight_boundary_prevents_query() {
    for after_version in [true, false] {
        let config = config();
        let flag = Arc::new(CancelFlag::default());
        let mut process = MockProcess::new(COMPONENT);
        let mut probe = MockProbe::default();
        if after_version {
            process.cancel_after = Some((1, flag.clone()));
        } else {
            probe.cancel = Some(flag.clone());
        }
        failure(
            Adb::new(&config, &process, &probe)
                .discover_apps("screen", &context(&FakeClock::default(), &*flag)),
            "cancelled",
        );
        assert_eq!(process.calls.lock().unwrap().len(), 1);
        assert_eq!(
            probe.calls.load(Ordering::SeqCst),
            usize::from(!after_version)
        );
    }
}

#[test]
fn cancellation_or_expiry_after_query_cannot_report_success() {
    let config = config();
    let flag = Arc::new(CancelFlag::default());
    let clock = Arc::new(FakeClock::default());
    let mut process = MockProcess::new(COMPONENT);
    process.cancel_after = Some((2, flag.clone()));
    failure(
        Adb::new(&config, &process, &MockProbe::default())
            .discover_apps("screen", &context(&*clock, &*flag)),
        "cancelled",
    );
    let mut process = MockProcess::new(COMPONENT);
    process.expire_after = Some((2, clock.clone()));
    failure(
        Adb::new(&config, &process, &MockProbe::default())
            .discover_apps("screen", &context(&*clock, &CancelFlag::default())),
        "expired",
    );
}

#[test]
fn parsing_checks_cancellation_while_processing_rows() {
    struct CancelDuringParse(AtomicUsize);
    impl Cancellation for CancelDuringParse {
        fn is_cancelled(&self) -> bool {
            self.0.fetch_add(1, Ordering::SeqCst) >= 4
        }
    }
    failure(
        parse_apps(
            &COMPONENT.repeat(10),
            MAX_OUTPUT_BYTES,
            &context(
                &FakeClock::default(),
                &CancelDuringParse(AtomicUsize::new(0)),
            ),
        ),
        "cancelled",
    );
}

#[test]
fn unsupported_device_protocol_mismatch_or_failed_query_stays_read_only() {
    let config = config();
    let process = MockProcess::new(COMPONENT);
    let probe = MockProbe::default();
    let clock = FakeClock::default();
    let flag = CancelFlag::default();
    let ctx = context(&clock, &flag);
    failure(
        Adb::new(&config, &process, &probe).discover_apps("unknown", &ctx),
        "unsupported",
    );
    assert!(process.calls.lock().unwrap().is_empty());
    let incompatible = MockProbe {
        version: 40,
        ..MockProbe::default()
    };
    failure(
        Adb::new(&config, &process, &incompatible).discover_apps("screen", &ctx),
        "adb_server_protocol_mismatch",
    );
    assert_eq!(process.calls.lock().unwrap().len(), 1);
    let process = MockProcess::new(COMPONENT);
    process.replies.lock().unwrap()[1].success = false;
    failure(
        Adb::new(&config, &process, &probe).discover_apps("screen", &ctx),
        "adb_read_failed",
    );
    assert!(process
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| !call.mutating));
    let process = MockProcess::new(COMPONENT);
    process.replies.lock().unwrap()[1].stdout = vec![0xff];
    let error = Adb::new(&config, &process, &probe)
        .discover_apps("screen", &ctx)
        .unwrap_err();
    assert_eq!(error, Failure::before("adb_read_failed"));
    assert_eq!(error.effect, Effect::None);
}
