use super::*;
use std::cell::Cell;
use tv_application::{Cancellation, Clock};
use tv_domain::Effect;
struct Time;
impl Clock for Time {
    fn monotonic_ms(&self) -> u64 {
        100
    }
    fn unix_ms(&self) -> u64 {
        100
    }
    fn sleep_ms(&self, _: u64) {
        panic!("no retries")
    }
}
struct Cancel(bool);
impl Cancellation for Cancel {
    fn is_cancelled(&self) -> bool {
        self.0
    }
}
struct Fake {
    calls: Cell<usize>,
    reply: Reply,
}
impl Transport for Fake {
    fn run_once(&self, session: &str, bytes: &[u8], _: &Context<'_>) -> PortResult<Reply> {
        self.calls.set(self.calls.get() + 1);
        assert_eq!(session, "one");
        assert_eq!(&bytes[..4], b"TVN1");
        assert!(bytes.ends_with("Tanya é🙂%s'".as_bytes()));
        Ok(self.reply)
    }
}
fn capability() -> Capability {
    Capability {
        sdk: 28,
        legacy_runner_verified: true,
        lifecycle_preflight_safe: true,
        public_hint_checks: true,
        action_set_text: true,
        device_session: "one".into(),
    }
}
fn request() -> Request {
    Request {
        device_session: "one".into(),
        observed_host_ms: 100,
        observed_device_uptime_ms: 500,
        package: "example.pkg".into(),
        resource: "example.pkg:id/editor".into(),
        role: "android.widget.EditText".into(),
        window: 1,
        bounds: [0, 0, 100, 50],
        before: "".into(),
        append: "Tanya é🙂%s'".into(),
    }
}
#[test]
fn explicit_native_preserves_utf8_and_dispatches_only_once() {
    let fake = Fake {
        calls: Cell::new(0),
        reply: Reply::VerifiedAccessibleText,
    };
    let ctx = Context {
        clock: &Time,
        cancellation: &Cancel(false),
        deadline_monotonic_ms: 1000,
    };
    assert!(append_once(
        Selection::ExplicitNativeText,
        &capability(),
        request(),
        &fake,
        &ctx
    )
    .is_ok());
    assert_eq!(fake.calls.get(), 1);
}
#[test]
fn capability_selection_session_staleness_cancel_gate_dispatch() {
    let fake = Fake {
        calls: Cell::new(0),
        reply: Reply::VerifiedAccessibleText,
    };
    let ctx = Context {
        clock: &Time,
        cancellation: &Cancel(false),
        deadline_monotonic_ms: 1000,
    };
    assert!(append_once(Selection::Disabled, &capability(), request(), &fake, &ctx).is_err());
    let mut cap = capability();
    cap.legacy_runner_verified = false;
    assert!(append_once(Selection::ExplicitNativeText, &cap, request(), &fake, &ctx).is_err());
    let mut req = request();
    req.device_session = "other".into();
    assert!(append_once(
        Selection::ExplicitNativeText,
        &capability(),
        req,
        &fake,
        &ctx
    )
    .is_err());
    let mut req = request();
    req.observed_host_ms = 101;
    assert!(append_once(
        Selection::ExplicitNativeText,
        &capability(),
        req,
        &fake,
        &ctx
    )
    .is_err());
    let ctx = Context {
        cancellation: &Cancel(true),
        ..ctx
    };
    assert!(append_once(
        Selection::ExplicitNativeText,
        &capability(),
        request(),
        &fake,
        &ctx
    )
    .is_err());
    assert_eq!(fake.calls.get(), 0);
}
#[test]
fn uncertain_is_not_retried() {
    let fake = Fake {
        calls: Cell::new(0),
        reply: Reply::Uncertain,
    };
    let ctx = Context {
        clock: &Time,
        cancellation: &Cancel(false),
        deadline_monotonic_ms: 1000,
    };
    let error = append_once(
        Selection::ExplicitNativeText,
        &capability(),
        request(),
        &fake,
        &ctx,
    )
    .unwrap_err();
    assert_eq!(error.effect, Effect::MayHaveChanged);
    assert_eq!(fake.calls.get(), 1);
}
#[test]
fn invalid_text_identity_and_expired_context_do_not_dispatch() {
    let fake = Fake {
        calls: Cell::new(0),
        reply: Reply::VerifiedAccessibleText,
    };
    let ctx = Context {
        clock: &Time,
        cancellation: &Cancel(false),
        deadline_monotonic_ms: 1000,
    };
    for append in ["".to_owned(), "x\n".to_owned(), "🙂".repeat(129)] {
        let mut req = request();
        req.append = append;
        assert!(append_once(
            Selection::ExplicitNativeText,
            &capability(),
            req,
            &fake,
            &ctx
        )
        .is_err());
    }
    let mut req = request();
    req.resource.clear();
    assert!(append_once(
        Selection::ExplicitNativeText,
        &capability(),
        req,
        &fake,
        &ctx
    )
    .is_err());
    let mut cap = capability();
    cap.sdk = 25;
    assert!(append_once(Selection::ExplicitNativeText, &cap, request(), &fake, &ctx).is_err());
    let ctx = Context {
        deadline_monotonic_ms: 100,
        ..ctx
    };
    assert!(append_once(
        Selection::ExplicitNativeText,
        &capability(),
        request(),
        &fake,
        &ctx
    )
    .is_err());
    assert_eq!(fake.calls.get(), 0);
}
#[test]
fn refusal_is_not_success_or_retry() {
    let fake = Fake {
        calls: Cell::new(0),
        reply: Reply::Refused,
    };
    let ctx = Context {
        clock: &Time,
        cancellation: &Cancel(false),
        deadline_monotonic_ms: 1000,
    };
    assert_eq!(
        append_once(
            Selection::ExplicitNativeText,
            &capability(),
            request(),
            &fake,
            &ctx
        )
        .unwrap_err()
        .effect,
        Effect::None
    );
    assert_eq!(fake.calls.get(), 1);
}

#[test]
fn unsafe_runner_lifecycle_is_not_a_capability() {
    let fake = Fake {
        calls: Cell::new(0),
        reply: Reply::VerifiedAccessibleText,
    };
    let ctx = Context {
        clock: &Time,
        cancellation: &Cancel(false),
        deadline_monotonic_ms: 1000,
    };
    let mut cap = capability();
    cap.lifecycle_preflight_safe = false;
    assert!(append_once(Selection::ExplicitNativeText, &cap, request(), &fake, &ctx).is_err());
    assert_eq!(fake.calls.get(), 0);
}

#[test]
fn validates_each_input_and_combined_size_before_allocation() {
    for (before, append) in [
        ("x".repeat(1_000_000), "a".into()),
        ("".into(), "x".repeat(1_000_000)),
        ("x".repeat(256), "a".into()),
        ("界".repeat(255), "🙂".into()),
        ("🙂".repeat(128), "a".into()),
    ] {
        assert!(validated_text_size(&before, &append).is_err());
    }
    assert_eq!(validated_text_size(&"界".repeat(255), "界"), Ok(768));
    assert_eq!(validated_text_size(&"🙂".repeat(127), "🙂"), Ok(512));
    assert_eq!(validated_text_size(&"x".repeat(255), "a"), Ok(256));
}

#[test]
fn oversized_before_and_append_never_dispatch() {
    let fake = Fake {
        calls: Cell::new(0),
        reply: Reply::VerifiedAccessibleText,
    };
    let ctx = Context {
        clock: &Time,
        cancellation: &Cancel(false),
        deadline_monotonic_ms: 1000,
    };
    for oversized_before in [true, false] {
        let mut req = request();
        if oversized_before {
            req.before = "x".repeat(1_000_000);
        } else {
            req.append = "x".repeat(1_000_000);
        }
        assert!(append_once(
            Selection::ExplicitNativeText,
            &capability(),
            req,
            &fake,
            &ctx
        )
        .is_err());
    }
    assert_eq!(fake.calls.get(), 0);
}
