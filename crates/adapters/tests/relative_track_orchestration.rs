//! Backend-neutral orchestration under the existing request-id and device reservation.
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tv_adapters::runtime::{CancelFlag, MemoryState};
use tv_application::*;
use tv_domain::*;
#[derive(Clone, Default)]
struct Time(Arc<AtomicU64>);
impl Clock for Time {
    fn unix_ms(&self) -> u64 {
        1000 + self.monotonic_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, n: u64) {
        self.0.fetch_add(n, Ordering::SeqCst);
    }
}
struct Device {
    states: Mutex<VecDeque<Observed>>,
    applied: Mutex<Vec<Action>>,
    cancel_after: Option<Arc<CancelFlag>>,
}
impl DeviceControl for Device {
    fn observe(&self, _: &str, _: &Context<'_>) -> PortResult<Observed> {
        let mut s = self.states.lock().unwrap();
        Ok(if s.len() > 1 {
            s.pop_front().unwrap()
        } else {
            s.front().unwrap().clone()
        })
    }
    fn apply(&self, _: &str, a: &Action, _: Option<&MediaPlan>, _: &Context<'_>) -> PortResult<()> {
        self.applied.lock().unwrap().push(a.clone());
        if let Some(c) = &self.cancel_after {
            c.cancel();
        }
        Ok(())
    }
}
#[derive(Default)]
struct Catalogue {
    after: Option<(Time, u64)>,
    unsupported: bool,
}
impl MediaCatalogue for Catalogue {
    fn resolve(&self, _: &str, _: &str, _: &Context<'_>) -> PortResult<MediaPlan> {
        panic!("no implicit resume")
    }
    fn validate_track(&self, _: &str, _: &Action, _: &Context<'_>) -> PortResult<()> {
        if let Some((time, n)) = &self.after {
            time.sleep_ms(*n);
        }
        if self.unsupported {
            Err(Failure::unsupported("track_selection_unsupported"))
        } else {
            Ok(())
        }
    }
    fn prepare(
        &self,
        _: &str,
        item: &str,
        source: &str,
        ms: u64,
        _: &Context<'_>,
    ) -> PortResult<MediaPlan> {
        if ms >= 20000 {
            return Err(Failure::before("media_position_out_of_range"));
        }
        Ok(MediaPlan {
            item_id: item.into(),
            source_id: Some(source.into()),
            media_kind: Some("video".into()),
            position_ticks: ms * MEDIA_TICKS_PER_MILLISECOND,
        })
    }
}
fn state(ms: u64) -> Observed {
    Observed {
        connected: true,
        playback: Playback::Paused,
        media_id: Some("item".into()),
        media_source_id: Some("source".into()),
        position_ticks: Some(ms * MEDIA_TICKS_PER_MILLISECOND),
        audio_index: Some(0),
        subtitle_index: Some(2),
        ..Observed::default()
    }
}
fn req(action: Action) -> Request {
    Request {
        id: "same".into(),
        device: "media_screen".into(),
        action,
        expires_at_unix_ms: 1500,
    }
}
fn seek(delta_ms: i64) -> Action {
    Action::SeekRelative {
        item_id: "item".into(),
        source_id: "source".into(),
        delta_ms,
    }
}
fn audio(index: u32) -> Action {
    Action::SelectAudioTrack {
        item_id: "item".into(),
        source_id: "source".into(),
        index,
    }
}
fn subtitle(index: i32) -> Action {
    Action::SelectSubtitleTrack {
        item_id: "item".into(),
        source_id: "source".into(),
        index,
    }
}
fn service(states: Vec<Observed>) -> Service<Device, Catalogue, MemoryState, Time> {
    Service {
        devices: Device {
            states: Mutex::new(states.into()),
            applied: Mutex::new(vec![]),
            cancel_after: None,
        },
        catalogue: Catalogue::default(),
        state: MemoryState::new(16, 10000),
        clock: Time::default(),
        policy: Policy {
            max_budget_ms: 1000,
            verification_poll_ms: 10,
            resume_position_tolerance_ms: 100,
        },
    }
}
#[test]
fn signed_seek_resolves_once_and_duplicate_never_recomputes() {
    for (delta, target) in [(2000, 7000), (-2000, 3000)] {
        let s = service(vec![state(5000), state(target)]);
        let r = s.execute(req(seek(delta)), &CancelFlag::default());
        assert_eq!(r.status, Status::Verified, "{}", r.code);
        assert_eq!(
            s.devices.applied.lock().unwrap().as_slice(),
            &[Action::SeekMedia {
                item_id: "item".into(),
                source_id: "source".into(),
                position_ms: target
            }]
        );
        assert!(s.execute(req(seek(delta)), &CancelFlag::default()).cached);
        assert_eq!(s.devices.applied.lock().unwrap().len(), 1);
        assert_eq!(
            s.execute(req(seek(delta + 1)), &CancelFlag::default()).code,
            "request_id_conflict"
        );
    }
}
#[test]
fn invalid_delta_bounds_missing_position_identity_and_duration_never_apply() {
    for (a, before, code) in [
        (seek(0), state(5), "invalid_request"),
        (seek(i64::MAX), state(5), "invalid_request"),
        (seek(i64::MIN), state(5), "invalid_request"),
        (seek(-6000), state(5000), "media_position_out_of_range"),
        (seek(15000), state(5000), "media_position_out_of_range"),
        (
            seek(1),
            Observed {
                position_ticks: None,
                ..state(5000)
            },
            "seek_position_unavailable",
        ),
        (
            seek(1),
            Observed {
                media_source_id: Some("other".into()),
                ..state(5000)
            },
            "media_control_identity_mismatch",
        ),
        (audio(u32::MAX), state(5), "invalid_request"),
        (subtitle(-2), state(5), "invalid_request"),
    ] {
        let s = service(vec![before]);
        let r = s.execute(req(a), &CancelFlag::default());
        assert_eq!(r.code, code);
        assert!(s.devices.applied.lock().unwrap().is_empty());
    }
}
#[test]
fn track_verification_is_explicit_and_preserves_paused_state() {
    for (action, after) in [
        (
            audio(1),
            Observed {
                audio_index: Some(1),
                ..state(5)
            },
        ),
        (
            subtitle(-1),
            Observed {
                subtitle_index: Some(-1),
                ..state(5)
            },
        ),
    ] {
        let s = service(vec![state(5), after]);
        let r = s.execute(req(action.clone()), &CancelFlag::default());
        assert_eq!(r.status, Status::Verified, "{}", r.code);
        assert_eq!(s.devices.applied.lock().unwrap().as_slice(), &[action]);
        assert_eq!(r.observed.unwrap().playback, Playback::Paused);
    }
}
#[test]
fn index_unknown_mismatch_source_change_or_resume_cannot_verify_and_never_repair() {
    for after in [
        state(5),
        Observed {
            audio_index: None,
            ..state(5)
        },
        Observed {
            audio_index: Some(1),
            playback: Playback::Playing,
            ..state(5)
        },
        Observed {
            audio_index: Some(1),
            media_source_id: Some("other".into()),
            ..state(5)
        },
    ] {
        let s = service(vec![state(5), after]);
        let r = s.execute(req(audio(1)), &CancelFlag::default());
        assert_eq!(r.status, Status::OutcomeUnknown, "{}", r.code);
        assert_eq!(s.devices.applied.lock().unwrap().len(), 1);
        let mut replay = req(audio(1));
        replay.expires_at_unix_ms = s.clock.unix_ms() + 500;
        assert!(s.execute(replay, &CancelFlag::default()).cached);
        assert_eq!(s.devices.applied.lock().unwrap().len(), 1);
    }
    let s = service(vec![
        state(5),
        Observed {
            subtitle_index: None,
            ..state(5)
        },
    ]);
    assert_eq!(
        s.execute(req(subtitle(-1)), &CancelFlag::default()).status,
        Status::OutcomeUnknown
    );
}
#[test]
fn before_cancel_expiry_and_unsupported_noop_do_not_apply() {
    let s = service(vec![state(5)]);
    let flag = CancelFlag::default();
    flag.cancel();
    assert_eq!(s.execute(req(audio(1)), &flag).status, Status::Cancelled);
    assert!(s.devices.applied.lock().unwrap().is_empty());
    let mut s = service(vec![state(5)]);
    s.catalogue.unsupported = true;
    assert_eq!(
        s.execute(req(audio(0)), &CancelFlag::default()).status,
        Status::Unsupported
    );
    assert!(s.devices.applied.lock().unwrap().is_empty());
    let mut s = service(vec![state(5)]);
    s.catalogue.after = Some((s.clock.clone(), 500));
    assert_eq!(
        s.execute(req(audio(0)), &CancelFlag::default()).status,
        Status::Expired
    );
    assert!(s.devices.applied.lock().unwrap().is_empty());
}
#[test]
fn cancel_after_dispatch_is_uncertain_and_never_replayed() {
    let mut s = service(vec![state(5)]);
    let c = Arc::new(CancelFlag::default());
    s.devices.cancel_after = Some(c.clone());
    assert_eq!(s.execute(req(audio(1)), &*c).status, Status::OutcomeUnknown);
    c.0.store(false, Ordering::Release);
    assert!(s.execute(req(audio(1)), &*c).cached);
    assert_eq!(s.devices.applied.lock().unwrap().len(), 1);
}
