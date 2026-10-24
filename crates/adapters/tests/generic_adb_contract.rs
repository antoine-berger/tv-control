//! Generic ADB contracts, grouped by capability around one shared process harness.

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
    delay_next_ms: AtomicU64,
}
impl Mock {
    fn new(replies: &[&str]) -> Self {
        Self {
            replies: Mutex::new(replies.iter().map(|s| Ok(s.as_bytes().to_vec())).collect()),
            calls: Mutex::new(Vec::new()),
            delay_next_ms: AtomicU64::new(0),
        }
    }
}
impl Process for Mock {
    fn run(&self, call: Invocation, ctx: &Context<'_>) -> PortResult<Output> {
        ctx.check()?;
        self.calls.lock().unwrap().push(call);
        ctx.clock
            .sleep_ms(self.delay_next_ms.swap(0, Ordering::Relaxed));
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

#[path = "generic_adb_contract/apps.rs"]
mod apps;
#[path = "generic_adb_contract/labels.rs"]
mod labels;
#[path = "generic_adb_contract/media.rs"]
mod media;
#[path = "generic_adb_contract/power.rs"]
mod power;
#[path = "generic_adb_contract/remote.rs"]
mod remote;
#[path = "generic_adb_contract/screenshot.rs"]
mod screenshot;
#[path = "generic_adb_contract/standby.rs"]
mod standby;
#[path = "generic_adb_contract/ui.rs"]
mod ui;
#[path = "generic_adb_contract/volume.rs"]
mod volume;

#[path = "generic_adb_contract/text_result.rs"]
mod text_result;

#[path = "generic_adb_contract/app_details.rs"]
mod app_details;
#[path = "generic_adb_contract/input_diagnostics.rs"]
mod input_diagnostics;
#[path = "generic_adb_contract/inputs.rs"]
mod inputs;
#[path = "generic_adb_contract/remote_media.rs"]
mod remote_media;
