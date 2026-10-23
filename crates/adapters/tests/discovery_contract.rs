//! Synthetic wire evidence only. No sockets, network discovery or device access.
use std::{
    net::Ipv4Addr,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use tv_adapters::discovery::{
    dns::{self, SnapshotCache},
    fixture::FixtureDiscovery,
    parse_ssdp, receive_snapshot,
};
use tv_application::{
    onboarding::{discover, plan_setup, NetworkDiscovery},
    Cancellation, Clock, Context,
};
use tv_domain::{onboarding::*, PortResult};
fn ip() -> Ipv4Addr {
    Ipv4Addr::new(192, 0, 2, 10)
}
struct LateWake {
    time: AtomicU64,
    overshoot: u64,
    cancel: AtomicBool,
}
impl Clock for LateWake {
    fn unix_ms(&self) -> u64 {
        0
    }
    fn monotonic_ms(&self) -> u64 {
        self.time.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, ms: u64) {
        self.time.fetch_add(ms + self.overshoot, Ordering::SeqCst);
    }
}
impl Cancellation for LateWake {
    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}
#[test]
fn receive_window_ends_normally_after_a_late_wake_with_or_without_results() {
    for with_packet in [false, true] {
        let clock = LateWake {
            time: AtomicU64::new(0),
            overshoot: 120,
            cancel: AtomicBool::new(false),
        };
        let ctx = Context {
            clock: &clock,
            cancellation: &clock,
            deadline_monotonic_ms: 350,
        };
        let packet = response(dns::SERVICES[0], 5555, 120);
        let mut sent = false;
        let result = receive_snapshot(&interface(), 100, &ctx, |mdns, out| {
            if mdns && with_packet && !sent {
                sent = true;
                out[..packet.len()].copy_from_slice(&packet);
                Ok((packet.len(), (ip(), 5353).into()))
            } else {
                Err(std::io::ErrorKind::WouldBlock.into())
            }
        })
        .unwrap();
        assert_eq!(result.len(), usize::from(with_packet));
        assert!(clock.monotonic_ms() >= 100);
        assert!(clock.monotonic_ms() < 350);
    }
}
#[test]
fn receive_window_never_masks_a_real_deadline_or_cancellation() {
    let clock = LateWake {
        time: AtomicU64::new(0),
        overshoot: 120,
        cancel: AtomicBool::new(false),
    };
    let ctx = Context {
        clock: &clock,
        cancellation: &clock,
        deadline_monotonic_ms: 100,
    };
    assert_eq!(
        receive_snapshot(&interface(), 100, &ctx, |_, _| Err(
            std::io::ErrorKind::WouldBlock.into()
        ))
        .unwrap_err()
        .code,
        "expired"
    );
    clock.time.store(0, Ordering::SeqCst);
    assert_eq!(
        receive_snapshot(&interface(), 100, &ctx, |_, _| {
            clock.cancel.store(true, Ordering::SeqCst);
            Err(std::io::ErrorKind::WouldBlock.into())
        })
        .unwrap_err()
        .code,
        "cancelled"
    );
}
#[test]
fn ten_second_budget_keeps_short_receive_window_and_checks_cancellation() {
    for overshoot in [0, 300, 11_000] {
        let clock = LateWake {
            time: AtomicU64::new(0),
            overshoot,
            cancel: AtomicBool::new(false),
        };
        let ctx = Context {
            clock: &clock,
            cancellation: &clock,
            deadline_monotonic_ms: 10_000,
        };
        let result = receive_snapshot(&interface(), 2_000, &ctx, |_, _| {
            Err(std::io::ErrorKind::WouldBlock.into())
        });
        if overshoot < 10_000 {
            assert!(result.unwrap().is_empty());
            assert!(
                clock.monotonic_ms() < 3_000,
                "did not return after the short window"
            );
        } else {
            assert_eq!(result.unwrap_err().code, "expired");
        }
        clock.time.store(0, Ordering::SeqCst);
        clock.cancel.store(true, Ordering::SeqCst);
        assert_eq!(
            receive_snapshot(&interface(), 2_000, &ctx, |_, _| {
                panic!("cancelled search must not receive")
            })
            .unwrap_err()
            .code,
            "cancelled"
        );
    }
}
fn interface() -> LocalInterface {
    LocalInterface {
        name: "lan-fixture".into(),
        address: Ipv4Addr::new(192, 0, 2, 1),
        netmask: Ipv4Addr::new(255, 255, 255, 0),
    }
}
fn labels(labels: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for label in labels {
        out.push(label.len() as u8);
        out.extend(label.as_bytes());
    }
    out.push(0);
    out
}
fn name(s: &str) -> Vec<u8> {
    labels(&s.split('.').collect::<Vec<_>>())
}
fn response(service: &str, port: u16, ttl: u32) -> Vec<u8> {
    let mut out = vec![0, 0, 0x84, 0, 0, 0, 0, 2, 0, 0, 0, 0];
    out.extend(name(&format!("Display.{service}")));
    out.extend([0, 33, 0x80, 1]);
    out.extend(ttl.to_be_bytes());
    let mut payload = vec![0, 0, 0, 0];
    payload.extend(port.to_be_bytes());
    payload.extend(name("display.local"));
    out.extend((payload.len() as u16).to_be_bytes());
    out.extend(payload);
    out.extend(name("display.local"));
    out.extend([0, 1, 0x80, 1]);
    out.extend(120u32.to_be_bytes());
    out.extend([0, 4]);
    out.extend(ip().octets());
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
fn malformed_dns_and_compression_cycles_are_rejected() {
    let packet = response(dns::SERVICES[0], 5555, 120);
    for n in 0..packet.len() {
        assert!(dns::records(&packet[..n]).is_none(), "truncation {n}");
    }
    let mut cycle = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0xc0, 12];
    cycle.extend([0; 10]);
    assert!(dns::records(&cycle).is_none());
    let mut count = packet;
    count[6] = 0xff;
    assert!(dns::records(&count).is_none());
}
#[test]
fn pairing_is_never_a_control_endpoint() {
    let mut cache = SnapshotCache::default();
    cache.mdns(&response(dns::SERVICES[2], 37123, 120), ip(), 0);
    assert_eq!(
        plan_setup(cache.snapshot(0)[0].clone(), 0).readiness,
        SetupReadiness::PairingRequired
    );
    cache.mdns(&response(dns::SERVICES[1], 39123, 120), ip(), 0);
    let plan = plan_setup(cache.snapshot(0)[0].clone(), 0);
    assert_eq!(plan.endpoint, Some("192.0.2.10:39123".into()));
    assert_eq!(plan.readiness, SetupReadiness::AuthorizationUnverified);
}
#[test]
fn expired_srv_is_not_returned_after_longer_discovery_window() {
    let mut cache = SnapshotCache::default();
    cache.mdns(&response(dns::SERVICES[1], 39123, 1), ip(), 100);
    assert_eq!(cache.snapshot(1099).len(), 1);
    assert!(cache.snapshot(1100).is_empty());
    let mut cache = SnapshotCache::default();
    cache.mdns(&response(dns::SERVICES[1], 39123, 1), ip(), 100);
    let stale = cache.snapshot(100)[0].clone();
    assert_eq!(
        plan_setup(stale, 1100).readiness,
        SetupReadiness::DiscoveryExpired
    );
}
#[test]
fn old_goodbye_cannot_remove_new_dynamic_port() {
    let mut cache = SnapshotCache::default();
    for (port, ttl, time) in [(37123, 120, 0), (38123, 120, 10), (37123, 0, 20)] {
        cache.mdns(&response(dns::SERVICES[1], port, ttl), ip(), time);
    }
    assert_eq!(cache.snapshot(20)[0].services[0].port, Some(38123));
    cache.mdns(&response(dns::SERVICES[1], 38123, 0), ip(), 30);
    assert!(cache.snapshot(30).is_empty());
}
#[test]
fn srv_target_address_disagreement_is_unresolved() {
    let mut packet = response(dns::SERVICES[1], 39123, 120);
    let end = packet.len();
    packet[end - 1] = 20;
    let mut cache = SnapshotCache::default();
    cache.mdns(&packet, ip(), 0);
    let plan = plan_setup(cache.snapshot(0)[0].clone(), 0);
    assert!(plan.endpoint.is_none());
    assert_eq!(plan.readiness, SetupReadiness::AddressUnresolved);
}
#[test]
fn split_address_records_resolve_without_record_order_assumptions() {
    let packet = response(dns::SERVICES[1], 39123, 120);
    let a_size = name("display.local").len() + 14;
    let split = packet.len() - a_size;
    let mut srv = packet[..split].to_vec();
    srv[7] = 1;
    let mut address = packet[..12].to_vec();
    address[7] = 1;
    address.extend(&packet[split..]);
    for reverse in [false, true] {
        let mut cache = SnapshotCache::default();
        let records = if reverse {
            [&address, &srv]
        } else {
            [&srv, &address]
        };
        for p in records {
            cache.mdns(p, ip(), 0);
        }
        assert_eq!(cache.snapshot(0)[0].services[0].port, Some(39123));
    }
}
#[test]
fn wire_label_boundaries_prevent_false_services_and_identity_collisions() {
    let packet = response(dns::SERVICES[0], 5555, 120);
    let owner = name("Display._adb._tcp.local");
    let mut hostile = packet[..12].to_vec();
    hostile.extend(labels(&["Display._adb", "_tcp", "local"]));
    hostile.extend(&packet[12 + owner.len()..]);
    let mut cache = SnapshotCache::default();
    cache.mdns(&hostile, ip(), 0);
    assert!(cache.snapshot(0).is_empty());
    for label in ["Display.part", "Display\\part"] {
        let mut valid = packet[..12].to_vec();
        valid.extend(labels(&[label, "_adb", "_tcp", "local"]));
        valid.extend(&packet[12 + owner.len()..]);
        cache.mdns(&valid, ip(), 0);
    }
    let result = cache.snapshot(0);
    assert_eq!(result[0].services.len(), 2);
    assert_ne!(
        result[0].services[0].instance,
        result[0].services[1].instance
    );
}
fn dial() -> Candidate {
    parse_ssdp(b"HTTP/1.1 200 OK\r\nST: urn:dial-multiscreen-org:service:dial:1\r\nLOCATION: http://metadata.invalid/secret\r\nSERVER: Display\r\n\r\n",ip()).unwrap()
}
#[test]
fn dial_duplicates_do_not_exhaust_real_accumulator() {
    let mut cache = SnapshotCache::default();
    for _ in 0..64 {
        cache.dial(dial(), 0);
    }
    let mut second = dial();
    second.address = Ipv4Addr::new(192, 0, 2, 20);
    cache.dial(second, 0);
    assert_eq!(cache.snapshot(0).len(), 2);
}
#[test]
fn dial_does_not_imply_adb_or_fetch_untrusted_location() {
    let plan = plan_setup(dial(), 0);
    assert_eq!(plan.readiness, SetupReadiness::DebuggingRequired);
    assert!(plan.endpoint.is_none());
}
struct Port {
    candidates: Vec<Candidate>,
    advance: bool,
}
impl NetworkDiscovery for Port {
    fn interfaces(&self) -> PortResult<Vec<LocalInterface>> {
        Ok(vec![interface()])
    }
    fn discover(
        &self,
        _: &LocalInterface,
        _: u64,
        ctx: &Context<'_>,
    ) -> PortResult<Vec<Candidate>> {
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
            valid_until_monotonic_ms: None,
        }],
    }
}
#[test]
fn policy_deduplicates_sanitizes_and_limits_to_chosen_subnet() {
    let c = candidate(ip());
    let port = Port {
        candidates: vec![c.clone(), c, candidate(Ipv4Addr::new(198, 51, 100, 1))],
        advance: false,
    };
    let fake = Fake::new();
    let result = discover(&port, "lan-fixture@192.0.2.1", 100, &fake.ctx()).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].services.len(), 1);
    assert_eq!(result[0].id, "screen_192_0_2_10");
    assert!(!result[0].label.chars().any(|c| c.is_control()));
    assert_eq!(
        discover(&port, "missing", 100, &fake.ctx())
            .unwrap_err()
            .code,
        "interface_missing_or_ambiguous"
    );
}
#[test]
fn cancellation_and_deadline_bound_entire_discovery() {
    let fake = Fake::new();
    fake.cancel.store(true, Ordering::SeqCst);
    let port = Port {
        candidates: vec![],
        advance: false,
    };
    assert_eq!(
        discover(&port, "lan-fixture", 100, &fake.ctx())
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
        discover(&port, "lan-fixture", 100, &fake.ctx())
            .unwrap_err()
            .code,
        "expired"
    );
}
#[test]
fn offline_wire_fixture_uses_real_parser() {
    let data = serde_json::json!({"interfaces":[{"name":"lan-fixture","address":"192.0.2.1","netmask":"255.255.255.0"}],"packets":[{"protocol":"mdns","peer":"192.0.2.10","hex":response(dns::SERVICES[0],5555,120).iter().map(|b|format!("{b:02x}")).collect::<String>()}]});
    let fixture = FixtureDiscovery::from_json(&serde_json::to_vec(&data).unwrap()).unwrap();
    let fake = Fake::new();
    assert_eq!(
        discover(&fixture, "lan-fixture", 100, &fake.ctx())
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn ambiguous_ports_are_not_selected() {
    let mut c = candidate(ip());
    let mut other = c.services[0].clone();
    other.port = Some(5556);
    c.services.push(other);
    assert_eq!(
        plan_setup(c, 0).readiness,
        SetupReadiness::AmbiguousEndpoint
    );
}

#[test]
fn compressed_target_address_and_address_expiry_are_checked() {
    let original = response(dns::SERVICES[1], 39123, 120);
    let target_offset = 12 + name(&format!("Display.{}", dns::SERVICES[1])).len() + 10 + 6;
    let a_offset = original.len() - name("display.local").len() - 14;
    let mut compressed = original[..a_offset].to_vec();
    compressed.extend([0xc0 | ((target_offset >> 8) as u8), target_offset as u8]);
    compressed.extend(&original[a_offset + name("display.local").len()..]);
    let mut cache = SnapshotCache::default();
    cache.mdns(&compressed, ip(), 0);
    assert_eq!(cache.snapshot(0)[0].services[0].port, Some(39123));
    let end = compressed.len();
    compressed[end - 10..end - 6].copy_from_slice(&1u32.to_be_bytes());
    let mut cache = SnapshotCache::default();
    cache.mdns(&compressed, ip(), 0);
    assert_eq!(
        plan_setup(cache.snapshot(1000)[0].clone(), 1000).readiness,
        SetupReadiness::AddressUnresolved
    );
}
