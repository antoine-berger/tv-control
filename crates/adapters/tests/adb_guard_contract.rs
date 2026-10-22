use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::TcpListener,
    sync::Mutex,
};
use tv_adapters::{
    adb::Adb,
    adb_server::{ServerProbe, SmartSocketProbe},
    config::{Config, Device},
    process::{Invocation, Output, Process},
    runtime::{CancelFlag, SystemClock},
};
use tv_application::{Clock, Context};
use tv_domain::*;
struct ProcessMock {
    replies: Mutex<VecDeque<&'static str>>,
    calls: Mutex<Vec<Invocation>>,
}
impl ProcessMock {
    fn new(replies: Vec<&'static str>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            calls: Mutex::new(Vec::new()),
        }
    }
}
impl Process for ProcessMock {
    fn run(&self, call: Invocation, _: &Context<'_>) -> PortResult<Output> {
        self.calls.lock().unwrap().push(call);
        Ok(Output {
            stdout: self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected subprocess")
                .as_bytes()
                .to_vec(),
            success: true,
        })
    }
}
struct Compatible;
impl ServerProbe for Compatible {
    fn version(&self, _: &str, _: u16, _: &Context<'_>) -> PortResult<u32> {
        Ok(41)
    }
}
fn cfg() -> Config {
    Config::from_json(include_bytes!("../../../examples/config.json")).unwrap()
}
fn context<'a>(clock: &'a SystemClock, flag: &'a CancelFlag) -> Context<'a> {
    Context {
        clock,
        cancellation: flag,
        deadline_monotonic_ms: clock.monotonic_ms() + 3000,
    }
}
const VERSION: &str = "Android Debug Bridge version 1.0.41\n";
#[test]
fn globally_duplicate_media_tag_cannot_dispatch() {
    let config = cfg();
    let process = ProcessMock::new(vec![
        VERSION,
        "tag=player, package=org.other.player\ntag=player, package=org.example.player\n",
    ]);
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    let e = Adb::new(&config, &process, &Compatible)
        .apply("screen", &Action::Pause, &context(&clock, &flag))
        .unwrap_err();
    assert_eq!(e.code, "media_tag_ambiguous");
    assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
}
#[test]
fn malicious_resolved_component_cannot_dispatch() {
    let config = cfg();
    let process = ProcessMock::new(vec![VERSION, "org.example.player/.Main;reboot\n"]);
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    let e = Adb::new(&config, &process, &Compatible)
        .apply(
            "screen",
            &Action::OpenApp {
                app: "player".into(),
            },
            &context(&clock, &flag),
        )
        .unwrap_err();
    assert_eq!(e.code, "app_not_launchable");
    assert!(process.calls.lock().unwrap().iter().all(|c| !c.mutating));
}
#[test]
fn incompatible_mock_server_only_receives_readonly_version_probe() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0u8; 16];
        socket.read_exact(&mut request).unwrap();
        socket.write_all(b"OKAY00040028").unwrap();
        request
    });
    let mut config = cfg();
    if let Device::Adb { server_port, .. } = config.devices.get_mut("screen").unwrap() {
        *server_port = port;
    }
    let process = ProcessMock::new(vec![VERSION]);
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    let probe = SmartSocketProbe { poll_ms: 2 };
    let e = Adb::new(&config, &process, &probe)
        .observe("screen", &context(&clock, &flag))
        .unwrap_err();
    assert_eq!(e.code, "adb_server_protocol_mismatch");
    assert_eq!(server.join().unwrap(), *b"000chost:version");
    let calls = process.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].args, vec![std::ffi::OsString::from("version")]);
}
