use std::{collections::VecDeque, sync::Mutex};
use tv_adapters::{
    config::Config,
    http::{Http, HttpRequest, HttpResponse},
    jellyfin::Jellyfin,
    runtime::{CancelFlag, SystemClock},
};
use tv_application::{Clock, Context};
use tv_domain::*;
struct Mock {
    replies: Mutex<VecDeque<(u16, String)>>,
    sent: Mutex<Vec<HttpRequest>>,
}
impl Mock {
    fn new(replies: Vec<(u16, String)>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            sent: Mutex::new(vec![]),
        }
    }
}
impl Http for Mock {
    fn send(&self, r: HttpRequest, _: &Context<'_>) -> PortResult<HttpResponse> {
        self.sent.lock().unwrap().push(r);
        let (status, body) = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected request");
        Ok(HttpResponse {
            status,
            body: body.into_bytes(),
        })
    }
}
const VERSION: &str = r#"{"Version":"12.0.0"}"#;
fn session(can_seek: bool, source: &str, types: &str) -> String {
    serde_json::json!([{"Id":"session","DeviceId":"example_device","UserId":"example_user","SupportsRemoteControl":true,"SupportsMediaControl":true,"PlayableMediaTypes":[types],"NowPlayingItem":{"Id":"episode"},"PlayState":{"IsPaused":true,"PositionTicks":100,"MediaSourceId":source,"CanSeek":can_seek}}]).to_string()
}

fn apply(mock: &Mock, seek: bool) -> PortResult<()> {
    let cfg = Config::from_json(include_bytes!("../../../examples/config.json")).unwrap();
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    let action = if seek {
        Action::SeekMedia {
            item_id: "episode".into(),
            source_id: "source".into(),
            position_ms: 1000,
        }
    } else {
        Action::PlayItem {
            item_id: "episode".into(),
            source_id: "source".into(),
            position_ms: 1000,
        }
    };
    Jellyfin::new(&cfg, mock).apply(
        "media_screen",
        &action,
        Some(&MediaPlan {
            item_id: "episode".into(),
            source_id: Some("source".into()),
            media_kind: Some("video".into()),
            position_ticks: 10_000_000,
        }),
        &Context {
            clock: &clock,
            cancellation: &flag,
            deadline_monotonic_ms: clock.monotonic_ms() + 1000,
        },
    )
}
#[test]
fn exact_play_uses_explicit_item_source_and_position_once() {
    let mock = Mock::new(vec![
        (200, VERSION.into()),
        (200, session(true, "old", "Video")),
        (204, String::new()),
    ]);
    apply(&mock, false).unwrap();
    let sent = mock.sent.lock().unwrap();
    assert_eq!(sent.len(), 3);
    assert!(sent[2].mutating);
    assert_eq!(sent[2].path,"/Sessions/session/Playing?playCommand=PlayNow&itemIds=episode&mediaSourceId=source&startPositionTicks=10000000");
}
#[test]
fn exact_seek_requires_native_capability_and_current_source() {
    for (can_seek, source, code) in [
        (false, "source", "seek_unsupported"),
        (true, "other", "seek_media_identity_mismatch"),
    ] {
        let mock = Mock::new(vec![
            (200, VERSION.into()),
            (200, session(can_seek, source, "Video")),
        ]);
        assert_eq!(apply(&mock, true).unwrap_err().code, code);
        assert!(mock.sent.lock().unwrap().iter().all(|r| !r.mutating));
    }
    let mock = Mock::new(vec![
        (200, VERSION.into()),
        (200, session(true, "source", "Video")),
        (204, String::new()),
    ]);
    apply(&mock, true).unwrap();
    assert_eq!(
        mock.sent.lock().unwrap()[2].path,
        "/Sessions/session/Playing/Seek?seekPositionTicks=10000000"
    );
}
#[test]
fn incompatible_media_type_prevents_play_and_ambiguous_dispatch_is_not_retried() {
    let mock = Mock::new(vec![
        (200, VERSION.into()),
        (200, session(true, "source", "Audio")),
    ]);
    assert_eq!(
        apply(&mock, false).unwrap_err().code,
        "play_item_capability_unavailable"
    );
    let mock = Mock::new(vec![
        (200, VERSION.into()),
        (200, session(true, "source", "Video")),
        (500, String::new()),
    ]);
    assert_eq!(
        apply(&mock, false).unwrap_err().effect,
        Effect::MayHaveChanged
    );
    assert_eq!(mock.sent.lock().unwrap().len(), 3);
}
#[test]
fn prepare_rejects_wrong_source_unknown_duration_and_out_of_range() {
    for (source, duration, position, expected) in [
        ("source", Some(20_000_000), 1000, None),
        (
            "other",
            Some(20_000_000),
            1000,
            Some("media_source_not_found"),
        ),
        ("source", None, 1000, Some("media_position_out_of_range")),
        (
            "source",
            Some(10_000_000),
            1000,
            Some("media_position_out_of_range"),
        ),
    ] {
        let body=serde_json::json!({"Items":[{"Id":"episode","Name":"Pilot","Type":"Episode","MediaSources":[{"Id":"source","RunTimeTicks":duration}]}]}).to_string();
        let mock = Mock::new(vec![(200, VERSION.into()), (200, body)]);
        let cfg = Config::from_json(include_bytes!("../../../examples/config.json")).unwrap();
        let clock = SystemClock::default();
        let flag = CancelFlag::default();
        let result = Jellyfin::new(&cfg, &mock).prepare_media(
            "media_screen",
            "episode",
            source,
            position,
            &Context {
                clock: &clock,
                cancellation: &flag,
                deadline_monotonic_ms: clock.monotonic_ms() + 1000,
            },
        );
        match expected {
            Some(code) => assert_eq!(result.unwrap_err().code, code),
            None => {
                let plan = result.unwrap();
                assert_eq!(plan.source_id.as_deref(), Some("source"));
                assert_eq!(plan.position_ticks, 10_000_000);
            }
        }
        assert!(mock.sent.lock().unwrap().iter().all(|r| !r.mutating));
    }
}

#[test]
fn new_controls_require_explicit_media_control_capability() {
    for value in [serde_json::Value::Null, serde_json::Value::Bool(false)] {
        for seek in [false, true] {
            let mut body: serde_json::Value =
                serde_json::from_str(&session(true, "source", "Video")).unwrap();
            body[0]["SupportsMediaControl"] = value.clone();
            let mock = Mock::new(vec![(200, VERSION.into()), (200, body.to_string())]);
            assert_eq!(
                apply(&mock, seek).unwrap_err().code,
                "media_control_unsupported"
            );
            assert!(mock.sent.lock().unwrap().iter().all(|r| !r.mutating));
        }
    }
}
