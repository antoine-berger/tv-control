//! Native IPv4 discovery: one mDNS query and one SSDP query, selected interface only.
pub mod dns;
pub mod fixture;
use std::{
    ffi::CStr,
    io,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    os::fd::AsRawFd,
};
use tv_application::{onboarding::NetworkDiscovery, Context};
use tv_domain::{onboarding::*, Failure, PortResult};
pub const SSDP_QUERY: &[u8] = b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: urn:dial-multiscreen-org:service:dial:1\r\n\r\n";
pub struct LocalDiscovery;
/// Preserve numeric evidence without exposing arbitrary OS messages or packet contents.
pub fn io_failure(stage: &'static str, error: io::Error) -> Failure {
    Failure::before(stage).with_os_error(error.raw_os_error())
}
pub fn interfaces() -> PortResult<Vec<LocalInterface>> {
    let mut root = std::ptr::null_mut();
    // SAFETY: getifaddrs writes a linked list; a guard frees it exactly once.
    if unsafe { libc::getifaddrs(&mut root) } != 0 {
        return Err(Failure::before("interfaces_unavailable"));
    }
    struct Guard(*mut libc::ifaddrs);
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe { libc::freeifaddrs(self.0) }
        }
    }
    let _guard = Guard(root);
    let mut next = root;
    let mut result = Vec::new();
    while !next.is_null() && result.len() < 64 {
        // SAFETY: the list stays owned by guard, addresses are checked before casts.
        let item = unsafe { &*next };
        next = item.ifa_next;
        if item.ifa_addr.is_null() || item.ifa_netmask.is_null() || item.ifa_name.is_null() {
            continue;
        }
        if unsafe { (*item.ifa_addr).sa_family } as i32 != libc::AF_INET
            || item.ifa_flags & libc::IFF_UP as u32 == 0
            || item.ifa_flags & libc::IFF_LOOPBACK as u32 != 0
        {
            continue;
        }
        let addr = unsafe { &*(item.ifa_addr as *const libc::sockaddr_in) };
        let mask = unsafe { &*(item.ifa_netmask as *const libc::sockaddr_in) };
        let address = Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes());
        if !address.is_private() && !address.is_link_local() {
            continue;
        }
        let name = unsafe { CStr::from_ptr(item.ifa_name) }
            .to_string_lossy()
            .into_owned();
        if name.len() > 64 || name.chars().any(|c| c.is_control()) {
            continue;
        }
        result.push(LocalInterface {
            name,
            address,
            netmask: Ipv4Addr::from(mask.sin_addr.s_addr.to_ne_bytes()),
        });
    }
    result.sort_by(|a, b| (&a.name, a.address).cmp(&(&b.name, b.address)));
    result.dedup();
    Ok(result)
}
fn socket(interface: &LocalInterface) -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind((interface.address, 0))?;
    socket.set_nonblocking(true)?;
    socket.set_multicast_ttl_v4(1)?;
    let address = libc::in_addr {
        s_addr: u32::from_ne_bytes(interface.address.octets()),
    };
    // SAFETY: this live socket receives a correctly sized in_addr value.
    let code = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_IP,
            libc::IP_MULTICAST_IF,
            &address as *const _ as *const libc::c_void,
            std::mem::size_of_val(&address) as libc::socklen_t,
        )
    };
    if code != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(socket)
}
pub fn parse_ssdp(data: &[u8], peer: Ipv4Addr) -> Option<Candidate> {
    if data.len() > 8192 {
        return None;
    }
    let text = std::str::from_utf8(data).ok()?;
    let mut lines = text.lines();
    if lines.next()?.trim() != "HTTP/1.1 200 OK" {
        return None;
    }
    let mut dial = false;
    let mut label = "DIAL display".to_string();
    for line in lines.take(64) {
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("st")
                && value.trim() == "urn:dial-multiscreen-org:service:dial:1"
            {
                dial = true;
            }
            if key.eq_ignore_ascii_case("server") {
                label = value
                    .trim()
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(80)
                    .collect();
            }
        }
    }
    dial.then_some(Candidate {
        id: String::new(),
        label,
        address: peer,
        services: vec![AdvertisedService {
            kind: ServiceKind::Dial,
            port: None,
            instance: None,
            valid_until_monotonic_ms: None,
        }],
    })
}
impl NetworkDiscovery for LocalDiscovery {
    fn interfaces(&self) -> PortResult<Vec<LocalInterface>> {
        interfaces()
    }
    fn discover(
        &self,
        interface: &LocalInterface,
        window_end_monotonic_ms: u64,
        ctx: &Context<'_>,
    ) -> PortResult<Vec<Candidate>> {
        ctx.check()?;
        let mdns = socket(interface).map_err(|e| io_failure("discovery_socket_failed", e))?;
        mdns.set_multicast_ttl_v4(255)
            .map_err(|e| io_failure("mdns_socket_failed", e))?;
        let ssdp = socket(interface).map_err(|e| io_failure("discovery_socket_failed", e))?;
        mdns.send_to(&dns::query(), ("224.0.0.251", 5353))
            .map_err(|e| io_failure("mdns_send_failed", e))?;
        ctx.check()?;
        ssdp.send_to(SSDP_QUERY, ("239.255.255.250", 1900))
            .map_err(|e| io_failure("ssdp_send_failed", e))?;
        receive_snapshot(
            interface,
            window_end_monotonic_ms,
            ctx,
            |is_mdns, packet| {
                if is_mdns {
                    mdns.recv_from(packet)
                } else {
                    ssdp.recv_from(packet)
                }
            },
        )
    }
}
/// Receive-only bounded snapshot; datagram I/O is injected for deterministic contracts.
pub fn receive_snapshot(
    interface: &LocalInterface,
    window_end_monotonic_ms: u64,
    ctx: &Context<'_>,
    mut receive: impl FnMut(bool, &mut [u8]) -> io::Result<(usize, SocketAddr)>,
) -> PortResult<Vec<Candidate>> {
    ctx.check()?;
    let mut cache = dns::SnapshotCache::default();
    let mut packet = [0u8; 8193];
    let mut received = 0;
    while ctx.clock.monotonic_ms() < window_end_monotonic_ms && received < 256 {
        ctx.check()?;
        for is_mdns in [true, false] {
            match receive(is_mdns, &mut packet) {
                Ok((n, SocketAddr::V4(peer))) => {
                    received += 1;
                    if !interface.contains(*peer.ip()) || n > 8192 {
                        continue;
                    }
                    if is_mdns && peer.port() == 5353 {
                        cache.mdns(&packet[..n], *peer.ip(), ctx.clock.monotonic_ms());
                    } else if !is_mdns {
                        if let Some(candidate) = parse_ssdp(&packet[..n], *peer.ip()) {
                            cache.dial(candidate, ctx.clock.monotonic_ms());
                        }
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(io_failure("discovery_receive_failed", e)),
            }
        }
        ctx.wait(
            window_end_monotonic_ms
                .saturating_sub(ctx.clock.monotonic_ms())
                .min(5),
        )?;
    }
    ctx.check()?;
    Ok(cache.snapshot(ctx.clock.monotonic_ms()))
}
