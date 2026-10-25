use super::*;
use crate::runtime::{CancelFlag, SystemClock};
use serde_json::json;
use std::{net::TcpListener, sync::mpsc, thread};
use tv_application::Clock;

fn context<'a>(clock: &'a dyn Clock, cancel: &'a CancelFlag) -> Context<'a> {
    Context {
        clock,
        cancellation: cancel,
        deadline_monotonic_ms: clock.monotonic_ms() + 5000,
    }
}

fn serve(handler: impl FnOnce(TcpStream) + Send + 'static) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/health/mcp", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        let (mut socket, peer) = listener.accept().unwrap();
        assert!(peer.ip().is_loopback());
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
            assert!(request.len() <= 512);
        }
        let request = String::from_utf8(request).unwrap();
        assert!(request.starts_with("GET /health/mcp HTTP/1.0\r\n"));
        assert!(request.contains("\r\nConnection: close\r\n"));
        assert!(!request.to_lowercase().contains("authorization"));
        handler(socket);
    });
    (url, worker)
}

fn check_response(response: Vec<u8>) -> PortResult<Option<&'static str>> {
    let (url, worker) = serve(move |mut socket| {
        let _ = socket.write_all(&response);
    });
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let result = failure_code(&url, &context(&clock, &cancel));
    worker.join().unwrap();
    result
}

fn response(body: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn snapshot(status: &str, state: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": 1, "component": "mcp", "status": status, "state": state,
        "details": {"transport":"stdio", "channel":"main", "child_state":"running",
                    "initialize":{"ok":false}, "server_discover":{"ok":true}},
        "future_additive_field":{"ignored":true}
    }))
    .unwrap()
}

#[test]
fn endpoint_is_numeric_loopback_and_exact_literal_route_only() {
    for url in [
        "http://127.0.0.1:1/health/mcp",
        "http://127.19.2.3:65535/health/mcp",
        "http://[::1]:80/health/mcp",
    ] {
        assert!(address(url).unwrap().ip().is_loopback());
    }
    for url in [
        "http://localhost:8080/health/mcp",
        "http://127.0.0.1.example.com:8080/health/mcp",
        "http://198.51.100.20:8080/health/mcp",
        "http://192.0.2.1:8080/health/mcp",
        "http://0.0.0.0:8080/health/mcp",
        "http://[::]:8080/health/mcp",
        "http://[::ffff:127.0.0.1]:8080/health/mcp",
        "http://[::1%25lo]:8080/health/mcp",
        "http://[::1%0]:8080/health/mcp",
        "http://[::1%1]:8080/health/mcp",
        "http://2130706433:8080/health/mcp",
        "http://0177.0.0.1:8080/health/mcp",
        "http://0x7f000001:8080/health/mcp",
        "http://user:secret@127.0.0.1:8080/health/mcp",
        "http://127.0.0.1:8080@198.51.100.20/health/mcp",
        "http://127.0.0.1/health/mcp",
        "http://127.0.0.1:0/health/mcp",
        "http://127.0.0.1:65536/health/mcp",
        "https://127.0.0.1:8080/health/mcp",
        "file:///health/mcp",
        "http://127.0.0.1:8080/readyz",
        "http://127.0.0.1:8080/health/mcp/",
        "http://127.0.0.1:8080/health/mcp?details=true",
        "http://127.0.0.1:8080/health/mcp#fragment",
        "http://127.0.0.1:8080/%68ealth/mcp",
        "http://127.0.0.1:8080/../health/mcp",
        "http://127.0.0.1:8080\r\nInjected: yes/health/mcp",
        "http://127.0.0.1:8080 /health/mcp",
    ] {
        assert_eq!(
            address(url).unwrap_err().code,
            "tunnel_mcp_health_invalid_url",
            "{url}"
        );
    }
}

#[test]
fn passive_unobserved_and_legacy_or_modern_discovery_allow_startup() {
    for (status, state) in [
        ("unknown", "not_observed"),
        ("ok", "initialized"),
        ("ok", "server_discovered"),
        ("ok", "discovered"),
    ] {
        assert_eq!(
            check_response(response(&snapshot(status, state))).unwrap(),
            None
        );
    }
}

#[test]
fn observed_failure_and_closed_child_have_safe_specific_diagnostics() {
    for (status, state, code) in [
        ("degraded", "failed", "tunnel_mcp_discovery_failed"),
        ("degraded", "closed", "tunnel_mcp_child_closed"),
        ("disabled", "disabled", "tunnel_mcp_not_ready"),
        ("degraded", "discovered", "tunnel_mcp_not_ready"),
        ("ok", "failed", "tunnel_mcp_discovery_failed"),
    ] {
        assert_eq!(
            check_response(response(&snapshot(status, state))).unwrap(),
            Some(code)
        );
    }
}

#[test]
fn schema_component_and_state_are_validated_but_additive_fields_are_ignored() {
    for body in [
        br#"{"schema_version":2,"component":"mcp","status":"ok","state":"discovered"}"#.as_slice(),
        br#"{"schema_version":1,"component":"other","status":"ok","state":"discovered"}"#,
        br#"{"schema_version":1,"component":"mcp","status":"ok","state":"new_state"}"#,
        br#"{"schema_version":1,"component":"mcp","status":"ok","state":"not_observed"}"#,
        br#"{"schema_version":1,"component":"mcp","status":"unknown"}"#,
        br#"{"schema_version":1,"component":"mcp","status":true,"state":"discovered"}"#,
        b"not json",
    ] {
        assert_eq!(
            check_response(response(body)).unwrap_err().code,
            "tunnel_mcp_health_invalid_response"
        );
    }
}

#[test]
fn http10_close_delimited_official_responses_are_supported() {
    let mut bytes = b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n".to_vec();
    bytes.extend_from_slice(&snapshot("ok", "discovered"));
    assert_eq!(check_response(bytes).unwrap(), None);
}

#[test]
fn redirects_encoding_ambiguous_lengths_and_truncation_never_establish_health() {
    for (bytes, code) in [
        (b"HTTP/1.1 302 Found\r\nLocation: http://198.51.100.20/health/mcp\r\nContent-Length: 0\r\n\r\n".as_slice(), "tunnel_mcp_health_http_status"),
        (b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n", "tunnel_mcp_health_http_status"),
        (b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", "tunnel_mcp_health_invalid_response"),
        (b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\n\r\n", "tunnel_mcp_health_invalid_response"),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx", "tunnel_mcp_health_invalid_response"),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n{}", "tunnel_mcp_health_invalid_response"),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n{}", "tunnel_mcp_health_invalid_response"),
        (b"HTTP/1.1 200 OK\r\nContent-Length: +2\r\n\r\n{}", "tunnel_mcp_health_invalid_response"),
        (b"HTTP/1.1 200 OK\r\nmalformed\r\n\r\n{}", "tunnel_mcp_health_invalid_response"),
    ] {
        assert_eq!(check_response(bytes.to_vec()).unwrap_err().code, code);
    }
}

#[test]
fn declared_and_streamed_body_and_headers_have_independent_size_limits() {
    let too_large = vec![b' '; MAX_BODY + 1];
    assert_eq!(
        check_response(response(&too_large)).unwrap_err().code,
        "tunnel_mcp_health_response_too_large"
    );
    let mut no_length = b"HTTP/1.0 200 OK\r\n\r\n".to_vec();
    no_length.extend_from_slice(&too_large);
    assert_eq!(
        check_response(no_length).unwrap_err().code,
        "tunnel_mcp_health_response_too_large"
    );
    let mut header = b"HTTP/1.0 200 OK\r\nX-Large: ".to_vec();
    header.extend(vec![b'x'; MAX_HEADERS]);
    header.extend_from_slice(b"\r\n\r\n{}");
    assert_eq!(
        check_response(header).unwrap_err().code,
        "tunnel_mcp_health_response_too_large"
    );
    let mut maximum = snapshot("ok", "discovered");
    maximum.resize(MAX_BODY, b' ');
    assert_eq!(check_response(response(&maximum)).unwrap(), None);
}

fn stall(mut socket: TcpStream) {
    let mut byte = [0];
    // Closing the bounded client read must also end the fixture connection.
    assert_eq!(socket.read(&mut byte).unwrap(), 0);
}

#[test]
fn cancellation_during_a_blocked_read_is_preserved_and_prompt() {
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let (sent, received) = mpsc::channel();
    let (url, worker) = serve(move |socket| {
        sent.send(()).unwrap();
        stall(socket);
    });
    let started = Instant::now();
    thread::scope(|scope| {
        let cancel = &cancel;
        scope.spawn(move || {
            received.recv_timeout(Duration::from_secs(2)).unwrap();
            cancel.cancel();
        });
        assert_eq!(
            failure_code(&url, &context(&clock, cancel))
                .unwrap_err()
                .code,
            "cancelled"
        );
    });
    assert!(started.elapsed() < Duration::from_millis(500));
    worker.join().unwrap();
}

#[test]
fn application_deadline_is_preserved_and_no_connection_occurs_when_cancelled() {
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let (url, worker) = serve(stall);
    let mut ctx = context(&clock, &cancel);
    ctx.deadline_monotonic_ms = clock.monotonic_ms() + 40;
    assert_eq!(failure_code(&url, &ctx).unwrap_err().code, "expired");
    worker.join().unwrap();
    cancel.cancel();
    assert_eq!(failure_code("invalid", &ctx).unwrap_err().code, "cancelled");
}

struct FrozenClock;
impl Clock for FrozenClock {
    fn unix_ms(&self) -> u64 {
        0
    }
    fn monotonic_ms(&self) -> u64 {
        0
    }
    fn sleep_ms(&self, _: u64) {}
}

#[test]
fn total_wall_time_cap_survives_slow_drips_and_a_frozen_application_clock() {
    let cancel = CancelFlag::default();
    let (url, worker) = serve(|mut socket| {
        socket.write_all(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
        for _ in 0..200 {
            thread::sleep(Duration::from_millis(10));
            if socket.write_all(b" ").is_err() {
                break;
            }
        }
    });
    let started = Instant::now();
    assert_eq!(
        failure_code(&url, &context(&FrozenClock, &cancel))
            .unwrap_err()
            .code,
        "tunnel_mcp_health_timeout"
    );
    assert!(started.elapsed() >= MAX_TIME);
    assert!(started.elapsed() < Duration::from_secs(2));
    worker.join().unwrap();
}

#[test]
fn permission_errors_are_classified_by_kind_or_errno_without_error_text() {
    for error in [
        std::io::Error::from_raw_os_error(libc::EPERM),
        std::io::Error::from_raw_os_error(libc::EACCES),
        std::io::Error::new(ErrorKind::PermissionDenied, "private diagnostic omitted"),
    ] {
        let errno = error.raw_os_error();
        let failure = io_failure(error);
        assert_eq!(failure.code, "tunnel_mcp_health_permission_denied");
        assert_eq!(failure.os_error, errno);
    }
    for error in [
        std::io::Error::from_raw_os_error(libc::ECONNREFUSED),
        std::io::Error::other("Permission denied is only prose"),
    ] {
        assert_eq!(io_failure(error).code, "tunnel_mcp_health_unavailable");
    }
}

fn permission_http_status(code: u16) {
    let response = format!(
        "HTTP/1.1 {code} Fixture\r\nLocation: http://198.51.100.20/health/mcp\r\nContent-Length: 7\r\n\r\nprivate"
    );
    assert_eq!(
        check_response(response.into_bytes()).unwrap_err().code,
        "tunnel_mcp_health_permission_denied"
    );
}

#[test]
fn http401_authentication_refusal_is_a_permission_error() {
    permission_http_status(401);
}

#[test]
fn http403_authorization_refusal_is_a_permission_error() {
    permission_http_status(403);
}
