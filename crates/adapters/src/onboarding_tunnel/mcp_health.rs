//! Bounded, passive reads of the official runtime's advertised MCP observations.
//! This is deliberately not a general HTTP client: numeric loopback only, one
//! literal route, no DNS, proxies, redirects, credentials or protocol messages.
use serde_json::Value;
use std::{
    io::{ErrorKind, Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};
use tv_application::Context;
use tv_domain::{Failure, PortResult};

#[cfg(test)]
mod tests;

const MAX_BODY: usize = 24 * 1024; // Official schema-1 component response bound.
const MAX_HEADERS: usize = 8 * 1024;
const MAX_TIME: Duration = Duration::from_millis(1000);
const IO_SLICE: Duration = Duration::from_millis(10);

fn fail(code: &'static str) -> Failure {
    Failure::before(code)
}
fn io_failure(error: std::io::Error) -> Failure {
    let denied = error.kind() == ErrorKind::PermissionDenied
        || matches!(error.raw_os_error(), Some(libc::EPERM | libc::EACCES));
    fail(if denied {
        "tunnel_mcp_health_permission_denied"
    } else {
        "tunnel_mcp_health_unavailable"
    })
    .with_os_error(error.raw_os_error())
}

fn address(url: &str) -> PortResult<SocketAddr> {
    let invalid = || fail("tunnel_mcp_health_invalid_url");
    if url.len() > 128 || url.bytes().any(|c| c.is_ascii_control()) {
        return Err(invalid());
    }
    let authority = url
        .strip_prefix("http://")
        .and_then(|s| s.strip_suffix("/health/mcp"))
        .ok_or_else(invalid)?;
    if !authority
        .bytes()
        .all(|c| c.is_ascii_hexdigit() || matches!(c, b':' | b'.' | b'[' | b']'))
    {
        return Err(invalid());
    }
    // Reject user-info, escapes and scope identifiers before SocketAddr's
    // numeric-only parser; it never resolves names or integer/hex IPv4 forms.
    let address: SocketAddr = authority.parse().map_err(|_| invalid())?;
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err(invalid());
    }
    Ok(address)
}

struct Budget(Instant);
impl Budget {
    fn check(&self, ctx: &Context<'_>) -> PortResult<Duration> {
        ctx.check()?;
        let remaining = MAX_TIME
            .checked_sub(self.0.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| fail("tunnel_mcp_health_timeout"))?;
        Ok(remaining.min(Duration::from_millis(ctx.remaining_ms().max(1))))
    }
    fn wait(&self, ctx: &Context<'_>) -> PortResult<()> {
        let remaining = self.check(ctx)?;
        // A real elapsed-time cap also bounds fake/stalled application clocks.
        std::thread::sleep(remaining.min(IO_SLICE));
        self.check(ctx).map(|_| ())
    }
}

struct Headers {
    end: usize,
    content_length: Option<usize>,
}
fn headers(bytes: &[u8]) -> PortResult<Option<Headers>> {
    let end = bytes.windows(4).position(|s| s == b"\r\n\r\n");
    let Some(end) = end.map(|i| i + 4) else {
        return if bytes.len() >= MAX_HEADERS {
            Err(fail("tunnel_mcp_health_response_too_large"))
        } else {
            Ok(None)
        };
    };
    if end > MAX_HEADERS {
        return Err(fail("tunnel_mcp_health_response_too_large"));
    }
    let invalid = || fail("tunnel_mcp_health_invalid_response");
    let text = std::str::from_utf8(&bytes[..end]).map_err(|_| invalid())?;
    let mut lines = text[..text.len() - 4].split("\r\n");
    let mut status = lines.next().ok_or_else(invalid)?.splitn(3, ' ');
    if !matches!(status.next(), Some("HTTP/1.0" | "HTTP/1.1")) {
        return Err(fail("tunnel_mcp_health_http_status"));
    }
    match status.next() {
        Some("200") => {}
        Some("401" | "403") => return Err(fail("tunnel_mcp_health_permission_denied")),
        // Including redirects: never follow another location.
        _ => return Err(fail("tunnel_mcp_health_http_status")),
    }
    let mut content_length = None;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or_else(invalid)?;
        if name.is_empty()
            || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
            || value.bytes().any(|c| c.is_ascii_control() && c != b'\t')
        {
            return Err(invalid());
        }
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some()
                || value.is_empty()
                || !value.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(invalid());
            }
            let length = value.parse::<usize>().map_err(|_| invalid())?;
            if length > MAX_BODY {
                return Err(fail("tunnel_mcp_health_response_too_large"));
            }
            content_length = Some(length);
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            || (name.eq_ignore_ascii_case("content-encoding") && value != "identity")
        {
            // HTTP/1.0 plus Connection: close requests an unencoded body. This
            // avoids an extra chunk/decompression parser and its resource risks.
            return Err(invalid());
        }
    }
    Ok(Some(Headers {
        end,
        content_length,
    }))
}

fn read_snapshot(url: &str, ctx: &Context<'_>) -> PortResult<Vec<u8>> {
    ctx.check()?;
    let address = address(url)?;
    let budget = Budget(Instant::now());
    let connected =
        TcpStream::connect_timeout(&address, budget.check(ctx)?.min(Duration::from_millis(50)));
    budget.check(ctx)?;
    let mut stream = connected.map_err(io_failure)?;
    stream.set_nonblocking(true).map_err(io_failure)?;
    let request = format!(
        "GET /health/mcp HTTP/1.0\r\nHost: {address}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    let mut pending = request.as_bytes();
    while !pending.is_empty() {
        budget.check(ctx)?;
        let written = stream.write(pending);
        budget.check(ctx)?;
        match written {
            Ok(0) => return Err(fail("tunnel_mcp_health_unavailable")),
            Ok(n) => pending = &pending[n..],
            Err(error) if error.kind() == ErrorKind::WouldBlock => budget.wait(ctx)?,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(io_failure(error)),
        }
    }
    let mut response = Vec::new();
    let mut parsed = None;
    loop {
        budget.check(ctx)?;
        let mut buffer = [0; 2048];
        let read = stream.read(&mut buffer);
        budget.check(ctx)?;
        let count = match read {
            Ok(n) => n,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                budget.wait(ctx)?;
                continue;
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(io_failure(error)),
        };
        if response.len() + count > MAX_HEADERS + MAX_BODY {
            return Err(fail("tunnel_mcp_health_response_too_large"));
        }
        response.extend_from_slice(&buffer[..count]);
        if parsed.is_none() {
            parsed = headers(&response)?;
        }
        if let Some(Headers {
            end,
            content_length,
        }) = &parsed
        {
            let length = response.len() - end;
            if length > MAX_BODY {
                return Err(fail("tunnel_mcp_health_response_too_large"));
            }
            if content_length.is_some_and(|expected| length > expected)
                || (count == 0 && content_length.is_some_and(|expected| length != expected))
            {
                return Err(fail("tunnel_mcp_health_invalid_response"));
            }
            if count == 0 || *content_length == Some(length) {
                return Ok(response.split_off(*end));
            }
        } else if count == 0 {
            return Err(fail("tunnel_mcp_health_invalid_response"));
        }
    }
}

pub(super) fn failure_code(url: &str, ctx: &Context<'_>) -> PortResult<Option<&'static str>> {
    let body = read_snapshot(url, ctx)?;
    let value: Value =
        serde_json::from_slice(&body).map_err(|_| fail("tunnel_mcp_health_invalid_response"))?;
    ctx.check()?;
    if value.get("schema_version").and_then(Value::as_u64) != Some(1)
        || value.get("component").and_then(Value::as_str) != Some("mcp")
    {
        return Err(fail("tunnel_mcp_health_invalid_response"));
    }
    let status = value.get("status").and_then(Value::as_str);
    let state = value.get("state").and_then(Value::as_str);
    match (status, state) {
        // This is a startup gate, not proof of conversation exposure. Do not
        // require initialize.ok: modern server/discover has its own receipt.
        (Some("unknown"), Some("not_observed"))
        | (Some("ok"), Some("initialized" | "server_discovered" | "discovered")) => Ok(None),
        (_, Some("failed")) => Ok(Some("tunnel_mcp_discovery_failed")),
        (_, Some("closed")) => Ok(Some("tunnel_mcp_child_closed")),
        (Some("degraded" | "disabled"), _) => Ok(Some("tunnel_mcp_not_ready")),
        _ => Err(fail("tunnel_mcp_health_invalid_response")),
    }
}
