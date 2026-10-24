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
    pause_preconditions: Mutex<Vec<bool>>,
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
    fn apply(
        &self,
        _: &str,
        a: &Action,
        plan: Option<&MediaPlan>,
        _: &Context<'_>,
    ) -> PortResult<()> {
        self.applied.lock().unwrap().push(a.clone());
        self.pause_preconditions
            .lock()
            .unwrap()
            .push(plan.is_some_and(|p| p.require_paused));
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
            require_paused: false,
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
            pause_preconditions: Mutex::new(vec![]),
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

#[test]
fn ignored_small_relative_seek_must_not_verify_unchanged_paused_position() {
    for delta in [-50, 50] {
        let s = service(vec![state(5000), state(5000)]);
        let r = s.execute(req(seek(delta)), &CancelFlag::default());
        assert_eq!(
            r.status,
            Status::OutcomeUnknown,
            "delta={delta}: {}",
            r.code
        );
        assert_eq!(s.devices.applied.lock().unwrap().len(), 1);
    }
}

#[test]
fn small_relative_seek_wrong_direction_never_verifies() {
    for (delta, after) in [(-50, 5025), (50, 4975)] {
        let s = service(vec![state(5000), state(after)]);
        let r = s.execute(req(seek(delta)), &CancelFlag::default());
        assert_eq!(
            r.status,
            Status::OutcomeUnknown,
            "delta={delta}: {}",
            r.code
        );
        assert_eq!(s.devices.applied.lock().unwrap().len(), 1);
    }
}

#[test]
fn natural_playing_progress_cannot_prove_relative_seek_and_dispatch_is_refused() {
    for delta in [-50, 50] {
        let playing = |position| Observed {
            playback: Playback::Playing,
            ..state(position)
        };
        let s = service(vec![playing(5000), playing(5010), playing(5020)]);
        let r = s.execute(req(seek(delta)), &CancelFlag::default());
        assert_eq!(r.status, Status::Unsupported, "delta={delta}: {}", r.code);
        assert_eq!(r.code, "relative_seek_playing_unverifiable");
        assert!(s.devices.applied.lock().unwrap().is_empty());
    }
}

#[test]
fn exact_small_paused_relative_seek_verifies_the_requested_displacement_once() {
    for (delta, target) in [(-50, 4950), (50, 5050)] {
        let s = service(vec![state(5000), state(target)]);
        let r = s.execute(req(seek(delta)), &CancelFlag::default());
        assert_eq!(r.status, Status::Verified, "delta={delta}: {}", r.code);
        assert_eq!(
            s.devices.pause_preconditions.lock().unwrap().as_slice(),
            &[true]
        );
        assert_eq!(
            s.devices.applied.lock().unwrap().as_slice(),
            &[Action::SeekMedia {
                item_id: "item".into(),
                source_id: "source".into(),
                position_ms: target,
            }]
        );
    }
}

#[test]
fn discarding_a_native_fraction_is_not_proof_of_the_requested_relative_displacement() {
    for delta in [-1, 1] {
        let mut before = state(5000);
        before.position_ticks = Some(50_005_000); // 5000.5 ms, not an integral millisecond.
        let s = service(vec![before, state(5000)]);
        let r = s.execute(req(seek(delta)), &CancelFlag::default());
        assert_eq!(
            r.status,
            Status::OutcomeUnknown,
            "delta={delta}: {}",
            r.code
        );
        assert_eq!(s.devices.applied.lock().unwrap().len(), 1);
    }
}

#[test]
fn relative_seek_cannot_verify_changed_pause_or_source_and_never_repairs() {
    for after in [
        Observed {
            playback: Playback::Playing,
            ..state(4950)
        },
        Observed {
            media_source_id: Some("changed".into()),
            ..state(4950)
        },
    ] {
        let s = service(vec![state(5000), after]);
        assert_eq!(
            s.execute(req(seek(-50)), &CancelFlag::default()).status,
            Status::OutcomeUnknown
        );
        assert_eq!(s.devices.applied.lock().unwrap().len(), 1);
    }
}

#[test]
fn native_fraction_can_verify_only_after_exact_resolved_target_is_observed() {
    for (delta, target) in [(-50, 4950), (50, 5050)] {
        let mut before = state(5000);
        before.position_ticks = Some(50_005_000);
        let s = service(vec![before, state(target)]);
        assert_eq!(
            s.execute(req(seek(delta)), &CancelFlag::default()).status,
            Status::Verified
        );
        assert_eq!(s.devices.applied.lock().unwrap().len(), 1);
    }
}

fn assert_raw_relative_result(
    before_ticks: u64,
    delta_ms: i64,
    tolerance_ms: u64,
    after_ticks: u64,
    expected: Status,
) {
    let mut before = state(0);
    before.position_ticks = Some(before_ticks);
    let mut after = state(0);
    after.position_ticks = Some(after_ticks);
    let mut s = service(vec![before, after]);
    s.policy.resume_position_tolerance_ms = tolerance_ms;
    let result = s.execute(req(seek(delta_ms)), &CancelFlag::default());
    assert_eq!(
        result.status, expected,
        "before={before_ticks} delta={delta_ms} tolerance={tolerance_ms} after={after_ticks}: {}",
        result.code
    );
    if expected == Status::OutcomeUnknown {
        let mut replay = req(seek(delta_ms));
        replay.expires_at_unix_ms = s.clock.unix_ms() + 500;
        let recovered = s.execute(replay, &CancelFlag::default());
        assert!(recovered.cached);
        assert_eq!(recovered.status, Status::OutcomeUnknown);
    }
    let target =
        (i128::from(before_ticks / MEDIA_TICKS_PER_MILLISECOND) + i128::from(delta_ms)) as u64;
    assert_eq!(
        s.devices.applied.lock().unwrap().as_slice(),
        &[Action::SeekMedia {
            item_id: "item".into(),
            source_id: "source".into(),
            position_ms: target,
        }]
    );
}

#[test]
fn one_native_tick_cannot_validate_a_seek_above_tolerance_boundary() {
    assert_raw_relative_result(50_009_999, 101, 100, 50_010_000, Status::OutcomeUnknown);
}

#[test]
fn raw_displacement_minimum_is_inclusive_at_t_plus_one_and_twice_tolerance() {
    for (before, delta, after, expected) in [
        (50_009_999, 101, 50_019_998, Status::OutcomeUnknown), // 9999 ticks, below 1 ms.
        (50_009_999, 101, 50_019_999, Status::Verified),       // Exactly 1 ms.
        (50_009_999, 200, 51_009_998, Status::OutcomeUnknown), // Below delta - tolerance.
        (50_009_999, 200, 51_009_999, Status::Verified),       // Exactly 100 ms.
        (50_000_000, -101, 49_990_001, Status::OutcomeUnknown),
        (50_000_000, -101, 49_990_000, Status::Verified),
        (50_000_001, -101, 49_990_000, Status::Verified), // Negative seek + native fraction.
        (50_009_999, -200, 48_000_000, Status::Verified), // Exact meaningful target.
        (50_009_999, 200, 52_000_000, Status::Verified),
    ] {
        assert_raw_relative_result(before, delta, 100, after, expected);
    }
}

#[test]
fn zero_tolerance_requires_full_raw_millisecond_displacement() {
    for (before, delta, after, expected) in [
        (50_009_999, 1, 50_010_000, Status::OutcomeUnknown),
        (50_000_000, 1, 50_010_000, Status::Verified),
        (50_000_001, -1, 49_990_000, Status::Verified),
        (50_005_000, 50, 50_500_000, Status::OutcomeUnknown),
        (50_005_000, -50, 49_500_000, Status::Verified),
    ] {
        assert_raw_relative_result(before, delta, 0, after, expected);
    }
}

#[test]
fn maximum_tolerance_keeps_one_millisecond_floor_without_overflow() {
    for (before, delta, after, expected) in [
        (50_009_999, 1, 50_010_000, Status::OutcomeUnknown),
        (50_000_000, 1, 50_010_000, Status::Verified),
        (50_005_000, 50, 50_500_000, Status::Verified),
        (50_005_000, -50, 49_500_000, Status::Verified),
    ] {
        assert_raw_relative_result(before, delta, u64::MAX, after, expected);
    }
}
