use super::*;
use crate::onboarding::presentation::display_label;
use std::sync::atomic::{AtomicU64, Ordering};
use tv_domain::onboarding::LocalInterface;
struct MenuClock(AtomicU64);
impl Clock for MenuClock {
    fn unix_ms(&self) -> u64 {
        0
    }
    fn monotonic_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}
#[test]
fn slow_interface_choice_does_not_age_a_later_fresh_advertisement() {
    // Simulate 31 seconds spent at the interface menu before any query/reply.
    let clock = MenuClock(AtomicU64::new(31_000));
    let port =
        FixtureDiscovery::from_json(include_bytes!("../../../../../examples/discovery.json"))
            .unwrap();
    let mut options = Options::default();
    options
        .values
        .insert("--interface".into(), "lan-fixture".into());
    let (_, candidates) = discover(&options, &port, &clock).unwrap();
    let candidate = candidates
        .into_iter()
        .find(|c| c.id == "screen_192_0_2_10")
        .unwrap();
    let plan = onboarding::plan_setup(candidate, clock.monotonic_ms());
    assert!(plan.endpoint.is_some());
    clock.sleep_ms(1000); // The user now confirms a still-fresh snapshot.
    assert_eq!(ensure_fresh(&plan, &clock), Ok(()));
    clock.sleep_ms(29_000);
    assert_eq!(
        ensure_fresh(&plan, &clock),
        Err("discovery_expired_rediscover")
    );
}
#[test]
fn search_window_and_hard_deadline_start_after_long_human_delay() {
    struct Probe;
    impl NetworkDiscovery for Probe {
        fn interfaces(&self) -> tv_domain::PortResult<Vec<LocalInterface>> {
            Ok(vec![LocalInterface {
                name: "lan".into(),
                address: Ipv4Addr::new(192, 0, 2, 1),
                netmask: Ipv4Addr::new(255, 255, 255, 0),
            }])
        }
        fn discover(
            &self,
            _: &LocalInterface,
            end: u64,
            ctx: &Context<'_>,
        ) -> tv_domain::PortResult<Vec<Candidate>> {
            assert_eq!(ctx.clock.monotonic_ms(), 31_000);
            assert_eq!(end, 33_000);
            assert_eq!(ctx.deadline_monotonic_ms, 41_000);
            ctx.check()?;
            Ok(vec![])
        }
    }
    let clock = MenuClock(AtomicU64::new(0));
    clock.sleep_ms(31_000);
    assert!(discover_on(&Options::default(), &Probe, &clock, "lan")
        .unwrap()
        .is_empty());
}
#[test]
fn discovery_timeout_configuration_is_bounded_and_independent() {
    let defaults = DiscoveryTiming::from_options(&Options::default()).unwrap();
    assert_eq!(defaults.window_ms, 2_000);
    assert_eq!(defaults.timeout_ms, 10_000);
    for timeout in [2_001, 5_000, 10_000] {
        let options = Options::parse(&["--timeout-ms".into(), timeout.to_string()]).unwrap();
        assert_eq!(
            DiscoveryTiming::from_options(&options).unwrap().timeout_ms,
            timeout
        );
    }
    for timeout in ["0", "1999", "2000", "10001", "18446744073709551615"] {
        let options = Options::parse(&["--timeout-ms".into(), timeout.into()]).unwrap();
        assert_eq!(
            DiscoveryTiming::from_options(&options).err().unwrap().code,
            "discovery_timeout_out_of_range"
        );
    }
    let options = Options::parse(&["--timeout-ms".into(), "invalid".into()]).unwrap();
    assert_eq!(
        DiscoveryTiming::from_options(&options).err().unwrap().code,
        "invalid_discovery_timeout"
    );
}
#[test]
fn search_results_must_complete_before_the_ten_second_deadline() {
    struct Probe(u64);
    impl NetworkDiscovery for Probe {
        fn interfaces(&self) -> tv_domain::PortResult<Vec<LocalInterface>> {
            Ok(vec![LocalInterface {
                name: "lan".into(),
                address: Ipv4Addr::new(192, 0, 2, 1),
                netmask: Ipv4Addr::new(255, 255, 255, 0),
            }])
        }
        fn discover(
            &self,
            _: &LocalInterface,
            end: u64,
            ctx: &Context<'_>,
        ) -> tv_domain::PortResult<Vec<Candidate>> {
            assert_eq!(end, 2_000);
            assert_eq!(ctx.deadline_monotonic_ms, 10_000);
            ctx.clock.sleep_ms(self.0);
            Ok(vec![])
        }
    }
    for elapsed in [3_000, 9_999, 10_000] {
        let clock = MenuClock(AtomicU64::new(0));
        let result = discover_on(&Options::default(), &Probe(elapsed), &clock, "lan");
        if elapsed < 10_000 {
            assert!(result.is_ok());
        } else {
            assert_eq!(result.unwrap_err().code, "expired");
        }
    }
}
#[test]
fn opaque_adb_and_ssdp_server_values_are_not_presented_as_friendly_names() {
    let mut candidate = Candidate {
        id: "screen".into(),
        label: "adb-deadbeef".into(),
        address: Ipv4Addr::new(192, 0, 2, 10),
        services: vec![AdvertisedService {
            kind: ServiceKind::AdbLegacy,
            port: Some(5555),
            instance: None,
            valid_until_monotonic_ms: None,
        }],
    };
    assert_eq!(display_label(&candidate), "Android device (ADB)");
    candidate.label = "Living room".into();
    assert_eq!(display_label(&candidate), "Living room");
    candidate.services[0].kind = ServiceKind::Dial;
    candidate.label = "Linux2.6 UPnP1.1 server1.1".into();
    assert_eq!(display_label(&candidate), "Unidentified media device");
}

fn discover(
    options: &Options,
    port: &dyn NetworkDiscovery,
    clock: &dyn Clock,
) -> Result<(String, Vec<Candidate>), &'static str> {
    let selected = select_interface(options, port)?;
    let candidates = discover_on(options, port, clock, &selected).map_err(fail)?;
    Ok((selected, candidates))
}
