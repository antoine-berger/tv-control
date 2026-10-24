//! Cross-language protocol golden and post-dispatch uncertainty tests.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use tv_application::{Cancellation, Clock};
use tv_domain::Effect;

struct MutableTime(AtomicU64);
impl Clock for MutableTime {
    fn monotonic_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn unix_ms(&self) -> u64 {
        self.monotonic_ms()
    }
    fn sleep_ms(&self, _: u64) {
        panic!("must not retry")
    }
}
struct MutableCancel(AtomicBool);
impl Cancellation for MutableCancel {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
fn capability() -> Capability {
    Capability {
        sdk: 28,
        legacy_runner_verified: true,
        lifecycle_preflight_safe: true,
        public_hint_checks: true,
        action_set_text: true,
        device_session: "fixture-session".into(),
    }
}
fn request() -> Request {
    Request {
        device_session: "fixture-session".into(),
        observed_host_ms: 100,
        observed_device_uptime_ms: 500,
        package: "example.pkg".into(),
        resource: "example.pkg:id/editor".into(),
        role: "android.widget.EditText".into(),
        window: 7,
        bounds: [-2, 3, 101, 55],
        before: "Pré🙂".into(),
        append: " Tanya %s'\"".into(),
    }
}
fn golden() -> Vec<u8> {
    let hex = include_str!("../../tests/protocol_golden.hex").trim();
    assert_eq!(hex.len() % 2, 0);
    (0..hex.len())
        .step_by(2)
        .map(|n| u8::from_str_radix(&hex[n..n + 2], 16).unwrap())
        .collect()
}
struct GoldenTransport(AtomicUsize);
impl Transport for GoldenTransport {
    fn run_once(&self, session: &str, payload: &[u8], _: &Context<'_>) -> PortResult<Reply> {
        self.0.fetch_add(1, Ordering::SeqCst);
        assert_eq!(session, "fixture-session");
        // Full message equality: header, expiry, every UTF-8 length/byte, window,
        // signed bounds, before/after and end-of-message, not a suffix assertion.
        assert_eq!(payload, golden());
        Ok(Reply::VerifiedAccessibleText)
    }
}
#[test]
fn full_payload_matches_shared_java_decoder_golden() {
    let time = MutableTime(AtomicU64::new(100));
    let cancel = MutableCancel(AtomicBool::new(false));
    let ctx = Context {
        clock: &time,
        cancellation: &cancel,
        deadline_monotonic_ms: 1000,
    };
    let transport = GoldenTransport(AtomicUsize::new(0));
    assert!(append_once(
        Selection::ExplicitNativeText,
        &capability(),
        request(),
        &transport,
        &ctx
    )
    .is_ok());
    assert_eq!(transport.0.load(Ordering::SeqCst), 1);
}

enum Boundary {
    Cancel,
    Expire,
    TransportError,
}
struct BoundaryTransport<'a> {
    boundary: Boundary,
    time: &'a MutableTime,
    cancel: &'a MutableCancel,
    calls: AtomicUsize,
}
impl Transport for BoundaryTransport<'_> {
    fn run_once(&self, _: &str, _: &[u8], _: &Context<'_>) -> PortResult<Reply> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.boundary {
            Boundary::Cancel => self.cancel.0.store(true, Ordering::SeqCst),
            Boundary::Expire => self.time.0.store(1000, Ordering::SeqCst),
            Boundary::TransportError => return Err(Failure::before("untrusted_transport_detail")),
        }
        // Even a verified reply cannot override a cancellation/deadline crossed
        // while the mutation was in progress.
        Ok(Reply::VerifiedAccessibleText)
    }
}
#[test]
fn cancel_deadline_and_error_during_transport_are_uncertain_without_replay() {
    for (boundary, code) in [
        (Boundary::Cancel, "cancelled"),
        (Boundary::Expire, "expired"),
        (Boundary::TransportError, "native_text_uncertain"),
    ] {
        let time = MutableTime(AtomicU64::new(100));
        let cancel = MutableCancel(AtomicBool::new(false));
        let ctx = Context {
            clock: &time,
            cancellation: &cancel,
            deadline_monotonic_ms: 1000,
        };
        let transport = BoundaryTransport {
            boundary,
            time: &time,
            cancel: &cancel,
            calls: AtomicUsize::new(0),
        };
        let failure = append_once(
            Selection::ExplicitNativeText,
            &capability(),
            request(),
            &transport,
            &ctx,
        )
        .unwrap_err();
        assert_eq!(failure.effect, Effect::MayHaveChanged);
        assert_eq!(failure.code, code);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }
}
