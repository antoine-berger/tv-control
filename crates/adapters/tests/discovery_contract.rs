//! All network evidence is synthetic. These tests never open a network socket.
use std::{
    net::Ipv4Addr,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use tv_adapters::discovery::{dns, fixture::FixtureDiscovery, parse_ssdp};
use tv_application::{
    onboarding::{discover, plan_setup, NetworkDiscovery},
    Cancellation, Clock, Context,
};
use tv_domain::{onboarding::*, PortResult};
fn interface() -> LocalInterface {
    LocalInterface {
        name: "lan-fixture".into(),
        address: Ipv4Addr::new(192, 0, 2, 1),
        netmask: Ipv4Addr::new(255, 255, 255, 0),
    }
}
fn name(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in s.split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out
}
fn response(service: &str, port: u16, ttl: u32) -> Vec<u8> {
    let mut out = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0];
    out.extend(name(&format!("Display.{service}")));
    out.extend([0, 33, 0x80, 1]);
    out.extend(ttl.to_be_bytes());
    let mut payload = vec![0, 0, 0, 0];
    payload.extend(port.to_be_bytes());
    payload.extend(name("display.local"));
    out.extend((payload.len() as u16).to_be_bytes());
    out.extend(payload);
    out
}
struct Fake {
    time: AtomicU64,
    cancel: AtomicBool,
}
impl Clock for Fake {
    fn unix_ms(&self) -> u64 {
        0
    }
    fn monotonic_ms(&self) -> u64 {
        self.time.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, n: u64) {
        self.time.fetch_add(n, Ordering::SeqCst);
    }
}
impl Cancellation for Fake {
    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}
impl Fake {
    fn new() -> Self {
        Self {
            time: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
        }
    }
    fn ctx(&self) -> Context<'_> {
        Context {
            clock: self,
            cancellation: self,
            deadline_monotonic_ms: 100,
        }
    }
}
#[test]
fn dns_rejects_every_truncation_and_compression_cycle() {
    let packet = response(dns::SERVICES[0], 5555, 120);
    for n in 0..packet.len() {
        assert!(
            dns::records(&packet[..n]).is_none(),
            "accepted truncation {n}"
        );
    }
    let mut cycle = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0xc0, 12];
    cycle.extend([0; 10]);
    assert!(dns::records(&cycle).is_none());
    let mut count = packet.clone();
    count[6] = 0xff;
    assert!(dns::records(&count).is_none());
}
#[test]
fn dns_preserves_identity_and_distinguishes_pairing_from_connect() {
    let ip = Ipv4Addr::new(192, 0, 2, 10);
    let mut result = Vec::new();
    dns::collect(&mut result, &response(dns::SERVICES[2], 37123, 120), ip);
    assert_eq!(
        plan_setup(result[0].clone()).readiness,
        SetupReadiness::PairingRequired
    );
    dns::collect(&mut result, &response(dns::SERVICES[1], 39123, 120), ip);
    let plan = plan_setup(result[0].clone());
    assert_eq!(plan.endpoint, Some("192.0.2.10:39123".into()));
    assert!(plan.candidate.services.iter().all(|s| s.instance.is_some()));
    assert_eq!(plan.readiness, SetupReadiness::AuthorizationUnverified);
}
#[test]
fn dns_goodbye_removes_service_and_new_port_replaces_old() {
    let ip = Ipv4Addr::new(192, 0, 2, 10);
    let mut result = Vec::new();
    for port in [37123, 38123] {
        dns::collect(&mut result, &response(dns::SERVICES[1], port, 120), ip);
    }
    assert_eq!(result[0].services.len(), 1);
    assert_eq!(result[0].services[0].port, Some(38123));
    dns::collect(&mut result, &response(dns::SERVICES[1], 38123, 0), ip);
    assert!(result.is_empty());
}
#[test]
fn ssdp_never_fetches_location_or_invents_adb_compatibility() {
    let ip = Ipv4Addr::new(192, 0, 2, 10);
    let value=parse_ssdp(b"HTTP/1.1 200 OK\r\nST: urn:dial-multiscreen-org:service:dial:1\r\nLOCATION: http://metadata.invalid/secret\r\nSERVER: Display\r\n\r\n",ip).unwrap();
    let plan = plan_setup(value);
    assert_eq!(plan.readiness, SetupReadiness::DebuggingRequired);
    assert!(plan.endpoint.is_none());
    assert!(parse_ssdp(b"HTTP/1.1 200 OK\r\nST: unrelated\r\n", ip).is_none());
}
struct Port {
    candidates: Vec<Candidate>,
    advance: bool,
}
impl NetworkDiscovery for Port {
    fn interfaces(&self) -> PortResult<Vec<LocalInterface>> {
        Ok(vec![interface()])
    }
    fn discover(&self, _: &LocalInterface, ctx: &Context<'_>) -> PortResult<Vec<Candidate>> {
        if self.advance {
            ctx.clock.sleep_ms(101);
        }
        Ok(self.candidates.clone())
    }
}
fn candidate(ip: Ipv4Addr) -> Candidate {
    Candidate {
        id: "untrusted".into(),
        label: "Display\x1b[31m\n$(command)".into(),
        address: ip,
        services: vec![AdvertisedService {
            kind: ServiceKind::AdbLegacy,
            port: Some(5555),
            instance: Some("screen._adb._tcp.local".into()),
        }],
    }
}
#[test]
fn discovery_deduplicates_and_sanitizes_without_leaving_selected_subnet() {
    let c = candidate(Ipv4Addr::new(192, 0, 2, 10));
    let port = Port {
        candidates: vec![c.clone(), c, candidate(Ipv4Addr::new(198, 51, 100, 1))],
        advance: false,
    };
    let fake = Fake::new();
    let result = discover(&port, "lan-fixture", &fake.ctx()).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].services.len(), 1);
    assert_eq!(result[0].id, "screen_192_0_2_10");
    assert!(!result[0].label.chars().any(|c| c.is_control()));
    assert_eq!(
        discover(&port, "missing", &fake.ctx()).unwrap_err().code,
        "interface_missing_or_ambiguous"
    );
}
#[test]
fn discovery_honors_cancellation_and_global_deadline() {
    let fake = Fake::new();
    fake.cancel.store(true, Ordering::SeqCst);
    let port = Port {
        candidates: vec![],
        advance: false,
    };
    assert_eq!(
        discover(&port, "lan-fixture", &fake.ctx())
            .unwrap_err()
            .code,
        "cancelled"
    );
    fake.cancel.store(false, Ordering::SeqCst);
    let port = Port {
        candidates: vec![],
        advance: true,
    };
    assert_eq!(
        discover(&port, "lan-fixture", &fake.ctx())
            .unwrap_err()
            .code,
        "expired"
    );
}
#[test]
fn offline_wire_fixture_uses_same_parser() {
    let data = serde_json::json!({"interfaces":[{"name":"lan-fixture","address":"192.0.2.1","netmask":"255.255.255.0"}],"packets":[{"protocol":"mdns","peer":"192.0.2.10","hex":response(dns::SERVICES[0],5555,120).iter().map(|b|format!("{b:02x}")).collect::<String>()}]});
    let fixture = FixtureDiscovery::from_json(&serde_json::to_vec(&data).unwrap()).unwrap();
    let fake = Fake::new();
    assert_eq!(
        discover(&fixture, "lan-fixture", &fake.ctx())
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn multiple_connect_ports_remain_ambiguous() {
    let mut c = candidate(Ipv4Addr::new(192, 0, 2, 10));
    let mut other = c.services[0].clone();
    other.port = Some(5556);
    c.services.push(other);
    let plan = plan_setup(c);
    assert_eq!(plan.readiness, SetupReadiness::AmbiguousEndpoint);
    assert!(plan.endpoint.is_none());
}

#[test]
fn reordered_service_records_and_case_variants_are_supported() {
    let ip = Ipv4Addr::new(192, 0, 2, 10);
    for reverse in [false, true] {
        let mut packets = vec![
            response(dns::SERVICES[0], 5555, 120),
            response("_ADB-TLS-CONNECT._TCP.LOCAL", 39123, 120),
        ];
        if reverse {
            packets.reverse();
        }
        let mut result = Vec::new();
        for packet in packets {
            dns::collect(&mut result, &packet, ip);
        }
        assert_eq!(
            plan_setup(result[0].clone()).endpoint,
            Some("192.0.2.10:39123".into())
        );
    }
}
#[test]
fn subnet_filters_network_broadcast_self_and_unrelated_addresses() {
    let lan = interface();
    for ip in [
        Ipv4Addr::new(192, 0, 2, 0),
        Ipv4Addr::new(192, 0, 2, 255),
        lan.address,
        Ipv4Addr::new(198, 51, 100, 10),
    ] {
        assert!(!lan.contains(ip));
    }
    assert!(lan.contains(Ipv4Addr::new(192, 0, 2, 10)));
}
