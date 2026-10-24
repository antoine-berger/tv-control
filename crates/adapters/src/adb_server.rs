//! Read-only smart-socket version probe. Never sends host:kill or starts a server.
use std::{
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr, TcpStream},
    os::fd::AsRawFd,
    time::Duration,
};
use tv_application::Context;
use tv_domain::*;
pub trait ServerProbe: Send + Sync {
    fn version(&self, host: &str, port: u16, ctx: &Context<'_>) -> PortResult<u32>;
}
pub struct SmartSocketProbe {
    pub poll_ms: u64,
}
impl ServerProbe for SmartSocketProbe {
    fn version(&self, host: &str, port: u16, ctx: &Context<'_>) -> PortResult<u32> {
        ctx.check()?;
        let ip: IpAddr = host
            .parse()
            .map_err(|_| Failure::before("invalid_adb_server"))?;
        if !ip.is_ipv4() || !ip.is_loopback() {
            return Err(Failure::before("invalid_adb_server"));
        }
        let mut stream = TcpStream::connect_timeout(
            &SocketAddr::new(ip, port),
            Duration::from_millis(ctx.remaining_ms().clamp(1, 50)),
        )
        .map_err(|_| Failure::before("adb_server_unavailable"))?;
        stream
            .set_nonblocking(true)
            .map_err(|_| Failure::before("adb_server_probe_failed"))?;
        let bytes = b"000chost:version";
        let mut sent = 0;
        while sent < bytes.len() {
            ctx.check()?;
            match stream.write(&bytes[sent..]) {
                Ok(0) => return Err(Failure::before("adb_server_probe_failed")),
                Ok(n) => sent += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    wait_socket(&stream, libc::POLLOUT, self.poll_ms, ctx)?
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Err(Failure::before("adb_server_probe_failed")),
            }
        }
        let mut response = [0u8; 12];
        let mut read = 0;
        while read < response.len() {
            ctx.check()?;
            match stream.read(&mut response[read..]) {
                Ok(0) => return Err(Failure::before("adb_server_probe_failed")),
                Ok(n) => read += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    wait_socket(&stream, libc::POLLIN, self.poll_ms, ctx)?
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Err(Failure::before("adb_server_probe_failed")),
            }
        }
        if &response[..8] != b"OKAY0004" {
            return Err(Failure::before("adb_server_probe_failed"));
        }
        u32::from_str_radix(
            std::str::from_utf8(&response[8..])
                .map_err(|_| Failure::before("adb_server_probe_failed"))?,
            16,
        )
        .map_err(|_| Failure::before("adb_server_probe_failed"))
    }
}

fn wait_socket(stream: &TcpStream, events: i16, poll_ms: u64, ctx: &Context<'_>) -> PortResult<()> {
    crate::process::wait_ready(
        &mut [libc::pollfd {
            fd: stream.as_raw_fd(),
            events,
            revents: 0,
        }],
        poll_ms,
        ctx,
    )
    .map_err(|failure| {
        if failure.code == "process_poll_failed" {
            Failure::before("adb_server_probe_failed")
        } else {
            failure
        }
    })
}
