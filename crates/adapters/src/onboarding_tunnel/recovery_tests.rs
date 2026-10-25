use super::*;
use crate::runtime::{CancelFlag, SystemClock};
use tv_application::Clock;

#[test]
fn diagnostic_conversion_is_closed_and_preserves_errors() {
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = Context {
        clock: &clock,
        cancellation: &cancel,
        deadline_monotonic_ms: clock.monotonic_ms() + 5000,
    };
    for code in [
        "tunnel_mcp_health_unavailable",
        "tunnel_mcp_health_timeout",
        "tunnel_mcp_health_http_status",
        "tunnel_mcp_health_invalid_response",
        "tunnel_mcp_health_response_too_large",
    ] {
        assert_eq!(health_diagnostic(before(code), &ctx).unwrap(), code);
        for errno in [libc::EPERM, libc::EACCES] {
            let error =
                health_diagnostic(before(code).with_os_error(Some(errno)), &ctx).unwrap_err();
            assert_eq!(error.code, code);
            assert_eq!(error.os_error, Some(errno));
        }
    }
    for code in [
        "tunnel_mcp_health_invalid_url",
        "tunnel_runtime_identity_mismatch",
        "tunnel_status_failed",
        "tunnel_mcp_health_permission_denied",
        "cancelled",
        "expired",
        "future_mcp_failure",
    ] {
        assert_eq!(
            health_diagnostic(before(code), &ctx).unwrap_err().code,
            code
        );
    }
}

#[test]
fn cancellation_or_deadline_after_a_diagnostic_stays_an_error() {
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let mut ctx = Context {
        clock: &clock,
        cancellation: &cancel,
        deadline_monotonic_ms: clock.monotonic_ms(),
    };
    assert_eq!(
        health_diagnostic(before("tunnel_mcp_health_unavailable"), &ctx)
            .unwrap_err()
            .code,
        "expired"
    );
    ctx.deadline_monotonic_ms = clock.monotonic_ms() + 5000;
    cancel.cancel();
    assert_eq!(
        health_diagnostic(before("tunnel_mcp_health_http_status"), &ctx)
            .unwrap_err()
            .code,
        "cancelled"
    );
}
