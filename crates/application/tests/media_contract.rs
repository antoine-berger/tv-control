use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tv_application::{media::*, Cancellation, Clock, Context};
use tv_domain::{media::*, PortResult};
struct Time(AtomicU64);
impl Clock for Time {
    fn unix_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn monotonic_ms(&self) -> u64 {
        self.unix_ms()
    }
    fn sleep_ms(&self, n: u64) {
        self.0.fetch_add(n, Ordering::SeqCst);
    }
}
struct Cancel(bool);
impl Cancellation for Cancel {
    fn is_cancelled(&self) -> bool {
        self.0
    }
}
struct Reader(AtomicUsize, bool);
impl tv_application::media::Reader for Reader {
    fn read(&self, _: &str, _: &Read, ctx: &Context<'_>) -> PortResult<Data> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if self.1 {
            ctx.clock.sleep_ms(500);
        }
        Ok(Data::Items {
            items: vec![],
            truncated: false,
        })
    }
}
fn request(read: Read) -> Request {
    Request {
        device: "screen".into(),
        expires_at_unix_ms: 200,
        read,
    }
}
#[test]
fn invalid_reads_never_reach_adapter() {
    let s = Service {
        reader: Reader(AtomicUsize::new(0), false),
        clock: Time(AtomicU64::new(100)),
        max_budget_ms: 1000,
    };
    for r in [
        Read::Search {
            query: " ".into(),
            limit: 1,
        },
        Read::Search {
            query: "ok".into(),
            limit: 51,
        },
        Read::Item {
            item_id: "../bad".into(),
        },
        Read::Episodes {
            series_id: "a".into(),
            season: Some(10001),
            limit: 1,
        },
    ] {
        assert_eq!(
            s.read(request(r), &Cancel(false)).unwrap_err().code,
            "invalid_media_read"
        );
    }
    assert_eq!(s.reader.0.load(Ordering::SeqCst), 0);
}
#[test]
fn cancelled_expired_and_overbudget_reads_do_not_dispatch() {
    let s = Service {
        reader: Reader(AtomicUsize::new(0), false),
        clock: Time(AtomicU64::new(100)),
        max_budget_ms: 1000,
    };
    assert_eq!(
        s.read(request(Read::Playback), &Cancel(true))
            .unwrap_err()
            .code,
        "cancelled"
    );
    let mut r = request(Read::Playback);
    r.expires_at_unix_ms = 100;
    assert_eq!(s.read(r, &Cancel(false)).unwrap_err().code, "expired");
    let mut r = request(Read::Playback);
    r.expires_at_unix_ms = 1101;
    assert_eq!(
        s.read(r, &Cancel(false)).unwrap_err().code,
        "budget_exceeds_policy"
    );
    assert_eq!(s.reader.0.load(Ordering::SeqCst), 0);
}
#[test]
fn read_completed_after_deadline_is_not_success() {
    let s = Service {
        reader: Reader(AtomicUsize::new(0), true),
        clock: Time(AtomicU64::new(100)),
        max_budget_ms: 1000,
    };
    assert_eq!(
        s.read(request(Read::Playback), &Cancel(false))
            .unwrap_err()
            .code,
        "expired"
    );
}
