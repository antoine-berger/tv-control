use std::{
    fs,
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use tv_adapters::{
    onboarding_runtime::{probe_local_mcp, AdbAuthorization, AdbRuntimeSpec, OwnedAdbRuntime},
    runtime::{CancelFlag, SystemClock},
};
use tv_application::{Clock, Context};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "tv runtime's {}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, format!("#!/usr/bin/python3\n{body}")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn context<'a>(clock: &'a SystemClock, cancel: &'a CancelFlag, budget: u64) -> Context<'a> {
    Context {
        clock,
        cancellation: cancel,
        deadline_monotonic_ms: clock.monotonic_ms() + budget,
    }
}
fn unused_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
const FAKE_ADB: &str = r#"
import json,os,pathlib,socket,sys
root=pathlib.Path(__file__).parent
with (root/'events').open('a') as f: f.write(json.dumps({'args':sys.argv[1:],'home':os.environ.get('HOME'),'ambient':os.environ.get('TV_RUNTIME_SECRET'),'mdns':os.environ.get('ADB_MDNS_AUTO_CONNECT')})+'\n')
if sys.argv[1:]==['version']: print('Android Debug Bridge version 1.0.41');sys.exit(0)
if '-L' in sys.argv:
    port=int(sys.argv[sys.argv.index('-L')+1].split(':')[-1]);s=socket.socket();s.bind(('127.0.0.1',port));s.listen()
    while True:
        conn,_=s.accept()
        with conn:
            prefix=conn.recv(4);data=conn.recv(int(prefix,16))
            with (root/'requests').open('ab') as f:f.write(data+b'\n')
            if data==b'host:version':response=b'0029'
            elif data==b'host:connect:192.0.2.10:5555':response=b'connected'
            elif data==b'host:devices':response=('192.0.2.10:5555\t'+(root/'state').read_text().strip()).encode()
            else:sys.exit(90)
            conn.sendall(b'OKAY'+format(len(response),'04x').encode()+response)
else:sys.exit(91)
"#;
fn spec(t: &Temp) -> AdbRuntimeSpec {
    AdbRuntimeSpec {
        executable: t.script("adb", FAKE_ADB),
        state_dir: t.0.join("runtime"),
        port: unused_port(),
        endpoint: "192.0.2.10:5555".into(),
        poll_ms: 2,
    }
}
#[test]
fn approval_is_required_before_any_identity_directory_or_child_exists() {
    let temp = Temp::new();
    let spec = spec(&temp);
    let state = spec.state_dir.clone();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    assert_eq!(
        OwnedAdbRuntime::start(spec, false, &context(&clock, &cancel, 5000))
            .err()
            .unwrap()
            .code,
        "adb_runtime_approval_required"
    );
    assert!(!state.exists());
    assert!(!temp.0.join("events").exists());
}
#[test]
fn owned_loopback_runtime_waits_for_actual_authorization_and_cleans_up() {
    let temp = Temp::new();
    let spec = spec(&temp);
    let port = spec.port;
    let home = spec.state_dir.join("adb-home");
    fs::write(temp.0.join("state"), "unauthorized").unwrap();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let mut runtime = OwnedAdbRuntime::start(spec, true, &context(&clock, &cancel, 5000)).unwrap();
    assert_eq!(
        runtime.connect(&context(&clock, &cancel, 5000)).unwrap(),
        AdbAuthorization::AwaitingAuthorization
    );
    fs::write(temp.0.join("state"), "device").unwrap();
    assert_eq!(
        runtime
            .authorization(&context(&clock, &cancel, 5000))
            .unwrap(),
        AdbAuthorization::Authorized
    );
    assert_eq!(runtime.port(), port);
    drop(runtime);
    assert!(TcpListener::bind(("127.0.0.1", port)).is_ok());
    let events: Vec<serde_json::Value> = fs::read_to_string(temp.0.join("events"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for event in events {
        assert_eq!(event["home"], home.to_str().unwrap());
        assert!(event["ambient"].is_null());
        assert_eq!(event["mdns"], "0");
        assert!(!event["args"].to_string().contains("host:kill"));
    }
    let requests = fs::read_to_string(temp.0.join("requests")).unwrap();
    assert!(requests
        .lines()
        .any(|line| line == "host:connect:192.0.2.10:5555"));
    assert!(requests.lines().any(|line| line == "host:devices"));
    assert!(!requests.contains("host:kill"));
}
#[test]
fn foreign_listener_and_existing_owned_session_are_not_adopted_or_stopped() {
    let temp = Temp::new();
    let mut first = spec(&temp);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    first.port = listener.local_addr().unwrap().port();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    assert_eq!(
        OwnedAdbRuntime::start(first, true, &context(&clock, &cancel, 5000))
            .err()
            .unwrap()
            .code,
        "adb_runtime_port_in_use"
    );
    assert!(listener.local_addr().is_ok());
    drop(listener);
    let first = spec(&temp);
    let second = first.clone();
    let runtime = OwnedAdbRuntime::start(first, true, &context(&clock, &cancel, 5000)).unwrap();
    assert_eq!(
        OwnedAdbRuntime::start(second, true, &context(&clock, &cancel, 5000))
            .err()
            .unwrap()
            .code,
        "adb_runtime_busy"
    );
    drop(runtime);
}
const FAKE_MCP: &str = r#"
import json,pathlib,select,sys
root=pathlib.Path(__file__).parent
def read():return json.loads(sys.stdin.readline())
def send(request,value):print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':value}),flush=True)
request=read();assert request['method']=='initialize'
# Before responding the peer must not have pipelined initialized/tools traffic.
assert not select.select([sys.stdin],[],[],0.05)[0]
send(request,{'protocolVersion':'2025-11-25','capabilities':{'tools':{}}})
assert read()['method']=='notifications/initialized'
request=read();assert request['method']=='tools/list';assert not select.select([sys.stdin],[],[],0.05)[0]
send(request,{'tools':[{'name':'tv_control','inputSchema':{'properties':{'action':{'oneOf':[{'properties':{'type':{'const':'observe'}}}]}}}}]})
request=read();assert request['params']['name']=='tv_capabilities'
if (root/'change-config').exists():pathlib.Path(sys.argv[2]).write_text('changed')
send(request,{'isError':False,'structuredContent':{'devices':[{'alias':'screen','actions':{'observe':True}}]}})
sys.stdin.read()
"#;
fn probe(
    temp: &Temp,
    script: &str,
    budget: u64,
) -> Result<tv_adapters::onboarding_runtime::LocalMcpObservation, tv_domain::Failure> {
    let binary = temp.script("mcp", script);
    let config = temp.0.join("config.json");
    fs::write(&config, b"configuration snapshot").unwrap();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    probe_local_mcp(
        &binary,
        &config,
        b"configuration snapshot",
        "screen",
        &context(&clock, &cancel, budget),
    )
}
#[test]
fn mcp_negotiates_sequentially_and_keeps_stdin_open_until_all_responses_arrive() {
    let temp = Temp::new();
    assert_eq!(probe(&temp, FAKE_MCP, 5000).unwrap().device, "screen");
}
#[test]
fn mcp_rejects_changed_snapshot_and_wrong_alias_without_forging_binding() {
    let temp = Temp::new();
    fs::write(temp.0.join("change-config"), b"1").unwrap();
    assert_eq!(
        probe(&temp, FAKE_MCP, 5000).unwrap_err().code,
        "mcp_probe_configuration_changed"
    );
    fs::remove_file(temp.0.join("change-config")).unwrap();
    assert_eq!(
        probe(
            &temp,
            &FAKE_MCP.replace("'alias':'screen'", "'alias':'other'"),
            5000
        )
        .unwrap_err()
        .code,
        "mcp_probe_device_unavailable"
    );
}
#[test]
fn mcp_deadline_kills_owned_child_without_waiting_for_eof_or_inherited_pipes() {
    let temp = Temp::new();
    let start = Instant::now();
    let error = probe(&temp, "import time;time.sleep(30)", 60).unwrap_err();
    assert_eq!(error.code, "expired");
    assert!(start.elapsed() < Duration::from_secs(1));
}
#[test]
fn cancelled_mcp_probe_does_not_spawn() {
    let temp = Temp::new();
    let binary = temp.script("mcp", "raise Exception('must not start')");
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    cancel.cancel();
    let error = probe_local_mcp(
        &binary,
        Path::new("/missing"),
        b"",
        "screen",
        &context(&clock, &cancel, 5000),
    )
    .unwrap_err();
    assert_eq!(error.code, "cancelled");
}

#[test]
fn mcp_protocol_errors_and_output_floods_are_not_success() {
    let temp = Temp::new();
    for (script, code) in [
        (
            "print('not json',flush=True)".to_string(),
            "mcp_probe_invalid_json",
        ),
        (
            "print('x'*70000,flush=True)".to_string(),
            "mcp_probe_output_limit",
        ),
        (
            FAKE_MCP.replace(
                "'protocolVersion':'2025-11-25'",
                "'protocolVersion':'unsupported'",
            ),
            "mcp_probe_incompatible_server",
        ),
        (
            FAKE_MCP.replace("'id':request['id']", "'id':'wrong-id'"),
            "mcp_probe_invalid_response",
        ),
    ] {
        assert_eq!(probe(&temp, &script, 5000).unwrap_err().code, code);
    }
}
