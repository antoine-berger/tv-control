//! Restart recovery binds the original delta/track selection, not a recomputed action.
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};
use tv_adapters::{
    operation_journal::OperationJournal,
    runtime::{CancelFlag, SystemClock},
};
use tv_application::{generic::LegacyGuard, Clock, Control};
use tv_domain::*;
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "tv-media-journal-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Fake(Arc<AtomicUsize>);
impl Control for Fake {
    fn execute(&self, r: Request, _: &dyn tv_application::Cancellation) -> Receipt {
        self.0.fetch_add(1, Ordering::SeqCst);
        Receipt {
            request_id: r.id,
            status: Status::Verified,
            code: "state_verified",
            observed: Some(Observed {
                connected: true,
                playback: Playback::Paused,
                media_id: Some("item".into()),
                media_source_id: Some("source".into()),
                audio_index: Some(1),
                subtitle_index: Some(-1),
                ..Observed::default()
            }),
            local_elapsed_ms: 1,
            cached: false,
        }
    }
}
fn action(kind: u8, source: &str, value: i64) -> Action {
    match kind {
        0 => Action::SeekRelative {
            item_id: "item".into(),
            source_id: source.into(),
            delta_ms: value,
        },
        1 => Action::SelectAudioTrack {
            item_id: "item".into(),
            source_id: source.into(),
            index: value as u32,
        },
        _ => Action::SelectSubtitleTrack {
            item_id: "item".into(),
            source_id: source.into(),
            index: value as i32,
        },
    }
}
#[test]
fn durable_restart_retains_identity_indices_and_exact_delta_without_dispatch() {
    for kind in 0..3 {
        let t = Temp::new();
        let path = t.0.join("journal");
        let count = Arc::new(AtomicUsize::new(0));
        let guard = || {
            LegacyGuard::new(
                Arc::new(Fake(count.clone())),
                Arc::new(OperationJournal::open(&path, 16, 60000).unwrap()),
                Arc::new(SystemClock::default()),
                BTreeMap::from([("media_screen".into(), "physical".into())]),
                10000,
            )
        };
        let request = Request {
            id: "select".into(),
            device: "media_screen".into(),
            action: action(kind, "source", 1),
            expires_at_unix_ms: SystemClock::default().unix_ms() + 5000,
        };
        let first = guard();
        assert_eq!(
            first
                .execute(request.clone(), &CancelFlag::default())
                .status,
            Status::Verified
        );
        drop(first);
        let restarted = guard();
        let result = restarted.execute(request.clone(), &CancelFlag::default());
        assert!(result.cached);
        let proof = result.observed.unwrap();
        assert_eq!(proof.audio_index, Some(1));
        assert_eq!(proof.subtitle_index, Some(-1));
        assert_eq!(proof.media_source_id.as_deref(), Some("source"));
        for changed in [action(kind, "different", 1), action(kind, "source", 2)] {
            let mut r = request.clone();
            r.action = changed;
            assert_eq!(
                restarted.execute(r, &CancelFlag::default()).code,
                "request_id_conflict"
            );
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}
