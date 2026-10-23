use std::{
    fs, io,
    net::{TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};
use tv_adapters::{
    onboarding_runtime::{probe_local_mcp, AdbAuthorization, AdbRuntimeSpec, OwnedAdbRuntime},
    runtime::{CancelFlag, SystemClock},
};
use tv_application::{Clock, Context};
// Parallel fork/exec can temporarily inherit another fixture's writable script
// or listener descriptors. Isolate fixture lifetimes; explicit concurrency tests
// still create their own children and exercise the production code unchanged.
static FIXTURE_LOCK: Mutex<()> = Mutex::new(());
struct Temp(PathBuf, #[allow(dead_code)] MutexGuard<'static, ()>);
impl Temp {
    fn new() -> Self {
        let lock = FIXTURE_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "tv runtime's {}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path, lock)
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
// Another test's fork may briefly retain this socket until its exec applies
// CLOEXEC. Require eventual refusal, while child exit/reaping checks stay strict.
fn assert_listener_eventually_closed(port: u16) -> usize {
    let deadline = Instant::now() + Duration::from_secs(2);
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut pending = 0;
    loop {
        match TcpStream::connect_timeout(&address, Duration::from_millis(50)) {
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => return pending,
            Err(error) => panic!("unexpected listener cleanup error: {error}"),
            Ok(stream) => drop(stream),
        }
        pending += 1;
        assert!(Instant::now() < deadline, "owned listener did not close");
        std::thread::sleep(Duration::from_millis(1));
    }
}

const FAKE_ADB: &str = r#"
import fcntl,json,os,pathlib,socket,sys
root=pathlib.Path(__file__).parent
with (root/'events').open('a') as f: f.write(json.dumps({'args':sys.argv[1:],'home':os.environ.get('HOME'),'ambient':os.environ.get('TV_RUNTIME_SECRET'),'mdns':os.environ.get('ADB_MDNS_AUTO_CONNECT'),'emu':os.environ.get('ADB_EMU')})+'\n')
if sys.argv[1:]==['version']: print('Android Debug Bridge version 1.0.41');sys.exit(0)
if '-L' in sys.argv:
    address=sys.argv[sys.argv.index('-L')+1]
    # Match official ADB socket activation: an inherited, listening socket, not a rebind.
    assert address.startswith('acceptfd:')
    fd=int(address.split(':')[1]);assert fd>2
    assert fcntl.fcntl(fd,fcntl.F_GETFD)&fcntl.FD_CLOEXEC==0
    s=socket.socket(fileno=fd)
    # The accept/version handshake below proves listening; Darwin rejects SO_ACCEPTCONN.
    assert s.getsockname()[0]=='127.0.0.1'
    with socket.socket() as competitor:
        try:competitor.bind(s.getsockname())
        except OSError:pass
        else:raise Exception('runtime port was not reserved')
    (root/'server-pid').write_text(str(os.getpid()))
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
fn inherited_listener_fixture_does_not_require_linux_only_socket_options() {
    let temp = Temp::new();
    let mut runtime_spec = spec(&temp);
    // Darwin defines SO_ACCEPTCONN but sogetoptlock returns ENOPROTOOPT for it:
    // https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/uipc_socket.c
    // Exercise that platform behavior even when this regression runs on Linux.
    let reject_acceptconn = r#"
import errno,socket
class PortableSocket(socket.socket):
    def getsockopt(self,level,option,*args):
        if level==socket.SOL_SOCKET and option==socket.SO_ACCEPTCONN:
            raise OSError(errno.ENOPROTOOPT,'fixture: Darwin does not expose SO_ACCEPTCONN')
        return super().getsockopt(level,option,*args)
socket.socket=PortableSocket
"#;
    runtime_spec.executable = temp.script("adb", &format!("{reject_acceptconn}{FAKE_ADB}"));
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let runtime = OwnedAdbRuntime::start(runtime_spec, true, &context(&clock, &cancel, 5000))
        .expect("an actual accepted version request must prove the inherited socket is listening");
    drop(runtime);
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
    let pid = fs::read_to_string(temp.0.join("server-pid"))
        .unwrap()
        .parse::<libc::pid_t>()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
    drop(runtime);
    // A closed accepted socket may retain this port in TIME_WAIT on Linux.
    // Rebinding is not evidence of listener cleanup; check the child and socket.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    assert_eq!(
        unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
        -1
    );
    assert_eq!(
        io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
    assert_listener_eventually_closed(port);
    let events: Vec<serde_json::Value> = fs::read_to_string(temp.0.join("events"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for event in events {
        assert_eq!(event["home"], home.to_str().unwrap());
        assert!(event["ambient"].is_null());
        assert_eq!(event["mdns"], "0");
        assert_eq!(event["emu"], "0");
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
fn listener_stays_reserved_when_owned_child_closes_its_copy() {
    let temp = Temp::new();
    let mut runtime_spec = spec(&temp);
    runtime_spec.executable = temp.script(
        "adb",
        &FAKE_ADB.replace(
            "            conn.sendall(b'OKAY'+format(len(response),'04x').encode()+response)",
            "            conn.sendall(b'OKAY'+format(len(response),'04x').encode()+response)\n            s.close();(root/'listener-closed').write_text('1')\n            import time;time.sleep(30)",
        ),
    );
    let port = runtime_spec.port;
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let runtime =
        OwnedAdbRuntime::start(runtime_spec, true, &context(&clock, &cancel, 5000)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !temp.0.join("listener-closed").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        TcpListener::bind(("127.0.0.1", port)).unwrap_err().kind(),
        io::ErrorKind::AddrInUse
    );
    let events: Vec<serde_json::Value> = fs::read_to_string(temp.0.join("events"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let address = events.last().unwrap()["args"][1].as_str().unwrap();
    let fd: i32 = address.strip_prefix("acceptfd:").unwrap().parse().unwrap();
    // The child inherited this descriptor, but future sibling execs must not.
    assert_eq!(
        unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
        libc::FD_CLOEXEC
    );
    drop(runtime);
    assert_listener_eventually_closed(port);
}
#[test]
fn cancellation_during_start_reaps_child_and_releases_reserved_listener() {
    let temp = Temp::new();
    let mut runtime_spec = spec(&temp);
    runtime_spec.executable = temp.script(
        "adb",
        &FAKE_ADB.replace(
            "    while True:",
            "    import time;time.sleep(30)\n    while True:",
        ),
    );
    let port = runtime_spec.port;
    let clock = SystemClock::default();
    let cancel = std::sync::Arc::new(CancelFlag::default());
    let child_cancel = cancel.clone();
    let pid_path = temp.0.join("server-pid");
    let notifier = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !pid_path.exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        child_cancel.cancel();
    });
    let error = OwnedAdbRuntime::start(runtime_spec, true, &context(&clock, &cancel, 5000))
        .err()
        .unwrap();
    notifier.join().unwrap();
    assert_eq!(error.code, "cancelled");
    let pid = fs::read_to_string(temp.0.join("server-pid"))
        .unwrap()
        .parse::<libc::pid_t>()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    assert_eq!(
        unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
        -1
    );
    assert_eq!(
        io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
    assert_listener_eventually_closed(port);
}
#[test]
fn cleanup_waits_for_sibling_fork_to_exec() {
    use std::{
        io::Read,
        os::{
            fd::AsRawFd,
            unix::{net::UnixStream, process::CommandExt},
        },
        process::Command,
    };
    let temp = Temp::new();
    let runtime_spec = spec(&temp);
    let port = runtime_spec.port;
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let runtime =
        OwnedAdbRuntime::start(runtime_spec, true, &context(&clock, &cancel, 5000)).unwrap();
    let events = fs::read_to_string(temp.0.join("events")).unwrap();
    let event: serde_json::Value = serde_json::from_str(events.lines().last().unwrap()).unwrap();
    let fd: i32 = event["args"][1]
        .as_str()
        .unwrap()
        .strip_prefix("acceptfd:")
        .unwrap()
        .parse()
        .unwrap();
    let (mut ready_parent, ready_child) = UnixStream::pair().unwrap();
    ready_parent
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let sibling = std::thread::spawn(move || {
        let mut command = Command::new("/bin/true");
        // SAFETY: only async-signal-safe write, poll, accept and close occur in
        // the child. Its inherited listener keeps the port alive until exec.
        unsafe {
            command.pre_exec(move || {
                let byte = [1u8];
                if libc::write(ready_child.as_raw_fd(), byte.as_ptr().cast(), 1) != 1 {
                    return Err(io::Error::last_os_error());
                }
                let mut poll = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                if libc::poll(&mut poll, 1, 2000) != 1 {
                    return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT));
                }
                let accepted = libc::accept(fd, std::ptr::null_mut(), std::ptr::null_mut());
                if accepted < 0 {
                    return Err(io::Error::last_os_error());
                }
                libc::close(accepted);
                Ok(())
            });
        }
        command.spawn().unwrap().wait().unwrap()
    });
    ready_parent.read_exact(&mut [0]).unwrap();
    drop(runtime);
    // The first cleanup probe releases the paused sibling into exec. An
    // immediate-refusal oracle fails; the bounded eventual oracle must pass.
    assert!(assert_listener_eventually_closed(port) > 0);
    assert!(sibling.join().unwrap().success());
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
