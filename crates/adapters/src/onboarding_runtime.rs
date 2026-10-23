//! Scoped, explicitly approved ADB supervision and sequential local MCP negotiation.
//! No global ADB daemon is started, adopted, stopped or reconfigured.
use crate::{
    adb_server::{ServerProbe, SmartSocketProbe},
    process::{nonblocking, BoundedProcess, Invocation, Process},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    net::{SocketAddrV4, TcpListener, TcpStream},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    time::Duration,
};
use tv_application::Context;
use tv_domain::{Failure, PortResult};

#[derive(Clone, Debug)]
pub struct AdbRuntimeSpec {
    pub executable: PathBuf,
    pub state_dir: PathBuf,
    pub port: u16,
    pub endpoint: String,
    pub poll_ms: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdbAuthorization {
    Authorized,
    AwaitingAuthorization,
    Connecting,
    Offline,
    Absent,
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        // This group was created by this exact child spawn, never a restored PID.
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct SessionLock(File);
impl Drop for SessionLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
fn private_directory(path: &Path) -> PortResult<()> {
    if !path.is_absolute() {
        return Err(Failure::before("runtime_path_not_absolute"));
    }
    match fs::create_dir(path) {
        Ok(()) => {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .map_err(|_| Failure::before("runtime_directory_failed"))?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(Failure::before("runtime_directory_failed")),
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|_| Failure::before("runtime_directory_failed"))?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(Failure::before("runtime_directory_not_private"));
    }
    Ok(())
}
fn executable(path: &Path) -> PortResult<PathBuf> {
    if !path.is_absolute() {
        return Err(Failure::before("runtime_executable_not_absolute"));
    }
    let path =
        fs::canonicalize(path).map_err(|_| Failure::before("runtime_executable_unavailable"))?;
    let metadata =
        fs::metadata(&path).map_err(|_| Failure::before("runtime_executable_unavailable"))?;
    if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
        return Err(Failure::before("runtime_executable_unavailable"));
    }
    Ok(path)
}
fn protocol(bytes: &[u8]) -> PortResult<u32> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix("Android Debug Bridge version 1.0.")
                    .and_then(|version| version.trim().parse().ok())
            })
        })
        .ok_or(Failure::before("adb_client_protocol_unavailable"))
}
pub struct OwnedAdbRuntime {
    spec: AdbRuntimeSpec,
    protocol: u32,
    connection_diagnostic: Option<String>,
    child: OwnedChild,
    // Retained after the child field so teardown kills/reaps before releasing the port.
    _listener: TcpListener,
    _lock: SessionLock,
}
impl OwnedAdbRuntime {
    /// Approval covers creating an isolated ADB identity and attempting this exact endpoint.
    /// The owner must retain this value for as long as the approved runtime is needed.
    pub fn start(mut spec: AdbRuntimeSpec, approved: bool, ctx: &Context<'_>) -> PortResult<Self> {
        ctx.check()?;
        if !approved {
            return Err(Failure::before("adb_runtime_approval_required"));
        }
        let endpoint = spec.endpoint.parse::<SocketAddrV4>().ok();
        if spec.port == 0
            || spec.poll_ms == 0
            || !endpoint.is_some_and(|endpoint| {
                endpoint.port() != 0
                    && !endpoint.ip().is_unspecified()
                    && !endpoint.ip().is_multicast()
                    && !endpoint.ip().is_broadcast()
            })
        {
            return Err(Failure::before("adb_runtime_invalid_configuration"));
        }
        spec.executable = executable(&spec.executable)?;
        private_directory(&spec.state_dir)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(spec.state_dir.join("runtime.lock"))
            .map_err(|_| Failure::before("adb_runtime_lock_failed"))?;
        let metadata = lock
            .metadata()
            .map_err(|_| Failure::before("adb_runtime_lock_failed"))?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(Failure::before("adb_runtime_lock_not_private"));
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(Failure::before("adb_runtime_busy"));
        }
        let lock = SessionLock(lock);
        let home = spec.state_dir.join("adb-home");
        private_directory(&home)?;
        let env = adb_environment(&home);
        let output = BoundedProcess {
            max_output: 16384,
            poll_ms: spec.poll_ms,
        }
        .run(
            Invocation {
                program: spec.executable.clone(),
                args: vec!["version".into()],
                env: env.clone(),
                stdin: Vec::new(),
                mutating: false,
            },
            ctx,
        )?;
        if !output.success {
            return Err(Failure::before("adb_client_protocol_unavailable"));
        }
        let protocol = protocol(&output.stdout)?;
        // Use ADB's socket-activation interface, retaining the exact bound listener.
        // Official contract: https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/android-s-beta-5/SOCKET-ACTIVATION.txt
        // A release/rebind gap would let a different server satisfy our version probe.
        let reservation = TcpListener::bind(("127.0.0.1", spec.port))
            .map_err(|_| Failure::before("adb_runtime_port_in_use"))?;
        // Keep the inherited descriptor above stdin/out/err even when the caller closed them.
        let listener_fd = unsafe { libc::fcntl(reservation.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if listener_fd < 0 {
            return Err(Failure::before("adb_runtime_listener_failed"));
        }
        // SAFETY: fcntl returned a new, exclusively owned socket descriptor.
        let listener = unsafe { TcpListener::from_raw_fd(listener_fd) };
        drop(reservation);
        let address = format!("acceptfd:{listener_fd}");
        ctx.check()?;
        let mut command = Command::new(&spec.executable);
        command
            .args([
                "-L",
                &address,
                "--one-device",
                "TVCONTROL_NO_USB",
                "server",
                "nodaemon",
            ])
            .env_clear()
            .envs(env)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        // SAFETY: the child callback only calls async-signal-safe fcntl. Clear CLOEXEC
        // in the forked child alone; other concurrently spawned children cannot inherit it.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(listener_fd, libc::F_GETFD);
                if flags < 0
                    || libc::fcntl(listener_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command
            .spawn()
            .map_err(|_| Failure::before("adb_runtime_spawn_failed"))?;
        let mut runtime = Self {
            spec,
            protocol,
            connection_diagnostic: None,
            child: OwnedChild(child),
            _listener: listener,
            _lock: lock,
        };
        loop {
            ctx.check()?;
            runtime.running()?;
            match (SmartSocketProbe {
                poll_ms: runtime.spec.poll_ms,
            })
            .version("127.0.0.1", runtime.spec.port, ctx)
            {
                Ok(actual) if actual == runtime.protocol => {
                    runtime.running()?;
                    return Ok(runtime);
                }
                Ok(_) => return Err(Failure::before("adb_server_protocol_mismatch")),
                Err(error) if error.code == "adb_server_unavailable" => {
                    ctx.wait(runtime.spec.poll_ms)?
                }
                Err(error) => return Err(error),
            }
        }
    }
    fn running(&mut self) -> PortResult<()> {
        match self
            .child
            .0
            .try_wait()
            .map_err(|_| Failure::before("adb_runtime_wait_failed"))?
        {
            None => Ok(()),
            Some(_) => Err(Failure::before("adb_runtime_stopped")),
        }
    }
    fn call(&mut self, request: &str, ctx: &Context<'_>) -> PortResult<Vec<u8>> {
        ctx.check()?;
        self.running()?;
        if (SmartSocketProbe {
            poll_ms: self.spec.poll_ms,
        })
        .version("127.0.0.1", self.spec.port, ctx)?
            != self.protocol
        {
            return Err(Failure::before("adb_server_protocol_mismatch"));
        }
        self.running()?;
        // Smart-socket requests do not contain the adb client's daemon-management
        // behavior: even a protocol mismatch can never issue host:kill/restart.
        let mut stream = TcpStream::connect_timeout(
            &format!("127.0.0.1:{}", self.spec.port).parse().unwrap(),
            Duration::from_millis(ctx.remaining_ms().clamp(1, 50)),
        )
        .map_err(|_| Failure::before("adb_runtime_unavailable"))?;
        stream
            .set_nonblocking(true)
            .map_err(|_| Failure::before("adb_runtime_request_failed"))?;
        let frame = format!("{:04x}{request}", request.len());
        let mut sent = 0;
        while sent < frame.len() {
            ctx.check()?;
            match stream.write(&frame.as_bytes()[sent..]) {
                Ok(0) => return Err(Failure::before("adb_runtime_request_failed")),
                Ok(n) => sent += n,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    ctx.wait(self.spec.poll_ms)?
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Err(Failure::before("adb_runtime_request_failed")),
            }
        }
        let mut header = [0; 8];
        socket_read(&mut stream, &mut header, self.spec.poll_ms, ctx)?;
        let rejected = &header[..4] == b"FAIL";
        if !rejected && &header[..4] != b"OKAY" {
            return Err(Failure::before("adb_runtime_response_invalid"));
        }
        let length = std::str::from_utf8(&header[4..])
            .ok()
            .and_then(|value| usize::from_str_radix(value, 16).ok())
            .filter(|length| *length <= 16384)
            .ok_or(Failure::before("adb_runtime_response_invalid"))?;
        let mut bytes = vec![0; length];
        socket_read(&mut stream, &mut bytes, self.spec.poll_ms, ctx)?;
        self.running()?;
        ctx.check()?;
        if rejected {
            return Err(Failure::before("adb_runtime_request_rejected"));
        }
        Ok(bytes)
    }
    pub fn connect(&mut self, ctx: &Context<'_>) -> PortResult<AdbAuthorization> {
        self.connection_diagnostic = None;
        let endpoint = self.spec.endpoint.clone();
        let acknowledgement = self.call(&format!("host:connect:{endpoint}"), ctx)?;
        // Diagnostics are terminal-safe and bounded, never authorization evidence.
        // In particular, "failed to authenticate" can accompany an unauthorized
        // transport that is still eligible for a TV-side approval.
        self.connection_diagnostic = Some(
            acknowledgement
                .iter()
                .flat_map(|byte| std::ascii::escape_default(*byte))
                .take(256)
                .map(char::from)
                .collect(),
        );
        // The connect acknowledgement is never authorization evidence.
        self.authorization(ctx)
    }
    /// Explicit user retry only. Keep this owned daemon and private identity;
    /// never disconnect other endpoints or invoke global daemon management.
    pub fn reconnect(&mut self, ctx: &Context<'_>) -> PortResult<AdbAuthorization> {
        self.connection_diagnostic = None;
        let endpoint = self.spec.endpoint.clone();
        match self.call(&format!("host:disconnect:{endpoint}"), ctx) {
            Ok(_) => {
                // The disconnect acknowledgement precedes asynchronous transport
                // removal. Reconnecting before absence can reuse the old session.
                while self.authorization(ctx)? != AdbAuthorization::Absent {
                    ctx.wait(self.spec.poll_ms)?;
                }
            }
            Err(error) if error.code == "adb_runtime_request_rejected" => {
                // Official ADB rejects disconnect when no such transport exists.
                // Require an independent exact-target absence, not error prose.
                if self.authorization(ctx)? != AdbAuthorization::Absent {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
        self.connect(ctx)
    }
    pub fn connection_diagnostic(&self) -> Option<&str> {
        self.connection_diagnostic.as_deref()
    }
    pub fn authorization(&mut self, ctx: &Context<'_>) -> PortResult<AdbAuthorization> {
        let result = self.call("host:devices", ctx)?;
        let text =
            std::str::from_utf8(&result).map_err(|_| Failure::before("adb_device_probe_failed"))?;
        let mut rows = text
            .lines()
            .filter(|line| line.split_whitespace().next() == Some(self.spec.endpoint.as_str()));
        let row = rows.next();
        if rows.next().is_some() {
            return Err(Failure::before("adb_device_probe_ambiguous"));
        }
        let state = if let Some(row) = row {
            let mut fields = row.split_whitespace().skip(1);
            let state = fields
                .next()
                .ok_or(Failure::before("adb_device_probe_failed"))?;
            if fields.next().is_some() {
                return Err(Failure::before("adb_device_probe_failed"));
            }
            Some(state)
        } else {
            None
        };
        ctx.check()?;
        Ok(match state {
            Some("device") => AdbAuthorization::Authorized,
            Some("unauthorized") => AdbAuthorization::AwaitingAuthorization,
            Some("connecting" | "authorizing") => AdbAuthorization::Connecting,
            Some("offline") => AdbAuthorization::Offline,
            None => AdbAuthorization::Absent,
            _ => return Err(Failure::before("adb_device_state_unsupported")),
        })
    }
    pub fn port(&self) -> u16 {
        self.spec.port
    }
}
fn socket_read(
    stream: &mut TcpStream,
    bytes: &mut [u8],
    poll_ms: u64,
    ctx: &Context<'_>,
) -> PortResult<()> {
    let mut received = 0;
    while received < bytes.len() {
        ctx.check()?;
        match stream.read(&mut bytes[received..]) {
            Ok(0) => return Err(Failure::before("adb_runtime_response_truncated")),
            Ok(n) => received += n,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => ctx.wait(poll_ms)?,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(Failure::before("adb_runtime_request_failed")),
        }
    }
    Ok(())
}
fn adb_environment(home: &Path) -> BTreeMap<std::ffi::OsString, std::ffi::OsString> {
    BTreeMap::from([
        ("HOME".into(), home.as_os_str().into()),
        ("ANDROID_USER_HOME".into(), home.as_os_str().into()),
        ("ADB_MDNS_AUTO_CONNECT".into(), "0".into()),
        ("ADB_MDNS".into(), "0".into()),
        ("ADB_USB".into(), "0".into()),
        ("ADB_EMU".into(), "0".into()),
        ("PATH".into(), "/usr/bin:/bin".into()),
    ])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalMcpObservation {
    pub device: String,
    pub protocol_version: String,
}
struct McpSession {
    _child: OwnedChild,
    input: ChildStdin,
    output: ChildStdout,
    errors: ChildStderr,
    pending: Vec<u8>,
    total: usize,
}
impl McpSession {
    fn send(&mut self, value: Value, ctx: &Context<'_>) -> PortResult<()> {
        let mut bytes =
            serde_json::to_vec(&value).map_err(|_| Failure::before("mcp_probe_encode_failed"))?;
        bytes.push(b'\n');
        let mut sent = 0;
        while sent < bytes.len() {
            ctx.check()?;
            self.drain_errors(ctx)?;
            match self.input.write(&bytes[sent..]) {
                Ok(0) => return Err(Failure::before("mcp_probe_input_closed")),
                Ok(n) => sent += n,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => ctx.wait(2)?,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Err(Failure::before("mcp_probe_input_failed")),
            }
        }
        Ok(())
    }
    fn budget(&mut self, n: usize) -> PortResult<()> {
        self.total = self.total.saturating_add(n);
        if self.total > 65536 {
            Err(Failure::before("mcp_probe_output_limit"))
        } else {
            Ok(())
        }
    }
    fn drain_errors(&mut self, ctx: &Context<'_>) -> PortResult<()> {
        let mut bytes = [0; 4096];
        loop {
            ctx.check()?;
            match self.errors.read(&mut bytes) {
                Ok(0) => return Ok(()),
                Ok(n) => self.budget(n)?,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Err(Failure::before("mcp_probe_pipe_failed")),
            }
        }
    }
    fn response(&mut self, id: &str, ctx: &Context<'_>) -> PortResult<Value> {
        loop {
            ctx.check()?;
            self.drain_errors(ctx)?;
            if let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = self.pending.drain(..=end).collect();
                let response: Value = serde_json::from_slice(&line)
                    .map_err(|_| Failure::before("mcp_probe_invalid_json"))?;
                if response.get("jsonrpc") != Some(&json!("2.0"))
                    || response.get("id") != Some(&json!(id))
                    || response.get("error").is_some()
                {
                    return Err(Failure::before("mcp_probe_invalid_response"));
                }
                return response
                    .get("result")
                    .cloned()
                    .ok_or(Failure::before("mcp_probe_invalid_response"));
            }
            let mut bytes = [0; 4096];
            match self.output.read(&mut bytes) {
                Ok(0) => return Err(Failure::before("mcp_probe_output_closed")),
                Ok(n) => {
                    self.budget(n)?;
                    self.pending.extend_from_slice(&bytes[..n]);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => ctx.wait(2)?,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Err(Failure::before("mcp_probe_pipe_failed")),
            }
        }
    }
}

/// Verify a local child against a caller-held exact configuration snapshot.
/// This returns protocol evidence only, not TV authorization or conversation identity.
pub fn probe_local_mcp(
    binary: &Path,
    configuration: &Path,
    expected_configuration: &[u8],
    device: &str,
    ctx: &Context<'_>,
) -> PortResult<LocalMcpObservation> {
    ctx.check()?;
    let binary = executable(binary)?;
    if !configuration.is_absolute()
        || !tv_domain::identifier(device)
        || expected_configuration.len() > 1_048_576
    {
        return Err(Failure::before("mcp_probe_invalid_configuration"));
    }
    let check_snapshot = || {
        if crate::setup::read_bounded(configuration).map_err(Failure::before)?
            != expected_configuration
        {
            return Err(Failure::before("mcp_probe_configuration_changed"));
        }
        Ok(())
    };
    check_snapshot()?;
    let child = Command::new(binary)
        .arg("mcp")
        .arg(configuration)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|_| Failure::before("mcp_probe_spawn_failed"))?;
    let mut child = OwnedChild(child);
    let input = child
        .0
        .stdin
        .take()
        .ok_or(Failure::before("mcp_probe_pipe_failed"))?;
    let output = child
        .0
        .stdout
        .take()
        .ok_or(Failure::before("mcp_probe_pipe_failed"))?;
    let errors = child
        .0
        .stderr
        .take()
        .ok_or(Failure::before("mcp_probe_pipe_failed"))?;
    for fd in [input.as_raw_fd(), output.as_raw_fd(), errors.as_raw_fd()] {
        nonblocking(fd).map_err(|_| Failure::before("mcp_probe_pipe_failed"))?;
    }
    let mut session = McpSession {
        _child: child,
        input,
        output,
        errors,
        pending: Vec::new(),
        total: 0,
    };
    session.send(json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"tv-control-onboarding","version":env!("CARGO_PKG_VERSION")}}}), ctx)?;
    let init = session.response("init", ctx)?;
    if init.get("protocolVersion") != Some(&json!("2025-11-25"))
        || !init
            .pointer("/capabilities/tools")
            .is_some_and(Value::is_object)
    {
        return Err(Failure::before("mcp_probe_incompatible_server"));
    }
    session.send(
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        ctx,
    )?;
    session.send(
        json!({"jsonrpc":"2.0","id":"tools","method":"tools/list","params":{}}),
        ctx,
    )?;
    let tools = session.response("tools", ctx)?;
    let tools = tools
        .get("tools")
        .and_then(Value::as_array)
        .ok_or(Failure::before("mcp_probe_invalid_tools"))?;
    let control: Vec<_> = tools
        .iter()
        .filter(|tool| tool.get("name") == Some(&json!("tv_control")))
        .collect();
    if control.len() != 1
        || !control[0]
            .pointer("/inputSchema/properties/action/oneOf")
            .and_then(Value::as_array)
            .is_some_and(|actions| {
                actions.iter().any(|action| {
                    action.pointer("/properties/type/const") == Some(&json!("observe"))
                })
            })
    {
        return Err(Failure::before("mcp_probe_observe_unavailable"));
    }
    session.send(json!({"jsonrpc":"2.0","id":"capabilities","method":"tools/call","params":{"name":"tv_capabilities","arguments":{}}}), ctx)?;
    let result = session.response("capabilities", ctx)?;
    let matches: Vec<_> = result
        .pointer("/structuredContent/devices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("alias") == Some(&json!(device)))
        .collect();
    if result.get("isError") != Some(&json!(false))
        || matches.len() != 1
        || matches[0].pointer("/actions/observe") != Some(&json!(true))
    {
        return Err(Failure::before("mcp_probe_device_unavailable"));
    }
    ctx.check()?;
    check_snapshot()?;
    // Session ownership keeps the process alive until every response has been read.
    Ok(LocalMcpObservation {
        device: device.into(),
        protocol_version: "2025-11-25".into(),
    })
}
