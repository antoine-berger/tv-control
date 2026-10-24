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
    replies: Mutex<VecDeque<String>>,
    sent: Mutex<Vec<HttpRequest>>,
    post_status: u16,
}
impl Mock {
    fn new(replies: Vec<String>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            sent: Mutex::new(vec![]),
            post_status: 204,
        }
    }
}
impl Http for Mock {
    fn send(&self, r: HttpRequest, ctx: &Context<'_>) -> PortResult<HttpResponse> {
        ctx.check()?;
        let post = r.mutating;
        self.sent.lock().unwrap().push(r);
        Ok(HttpResponse {
            status: if post { self.post_status } else { 200 },
            body: if post {
                vec![]
            } else {
                self.replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected read")
                    .into_bytes()
            },
        })
    }
}
const VERSION: &str = r#"{"Version":"12.0.0"}"#;
fn session(source: &str, commands: &[&str], audio: Option<i32>, subtitle: Option<i32>) -> String {
    serde_json::json!([{"Id":"session","DeviceId":"example_device","UserId":"example_user","SupportsRemoteControl":true,"SupportsMediaControl":true,"SupportedCommands":commands,"NowPlayingItem":{"Id":"item"},"PlayState":{"IsPaused":true,"PositionTicks":100,"MediaSourceId":source,"CanSeek":true,"AudioStreamIndex":audio,"SubtitleStreamIndex":subtitle}}]).to_string()
}
fn metadata(tracks: serde_json::Value) -> String {
    serde_json::json!({"Items":[{"Id":"item","Name":"Fixture","Type":"Episode","MediaSources":[{"Id":"source","MediaStreams":tracks}]}]}).to_string()
}
fn good_tracks() -> serde_json::Value {
    serde_json::json!([{"Index":0,"Type":"Audio"},{"Index":1,"Type":"Audio"},{"Index":2,"Type":"Subtitle"}])
}
fn audio(index: u32) -> Action {
    Action::SelectAudioTrack {
        item_id: "item".into(),
        source_id: "source".into(),
        index,
    }
}
fn sub(index: i32) -> Action {
    Action::SelectSubtitleTrack {
        item_id: "item".into(),
        source_id: "source".into(),
        index,
    }
}
fn apply(mock: &Mock, action: &Action) -> PortResult<()> {
    let cfg = Config::from_json(include_bytes!("../../../examples/config.json")).unwrap();
    let clock = SystemClock::default();
    let c = CancelFlag::default();
    Jellyfin::new(&cfg, mock).apply(
        "media_screen",
        action,
        None,
        &Context {
            clock: &clock,
            cancellation: &c,
            deadline_monotonic_ms: clock.monotonic_ms() + 1000,
        },
    )
}
fn replies(final_session: String, tracks: serde_json::Value) -> Vec<String> {
    vec![
        VERSION.into(),
        session(
            "source",
            &["SetAudioStreamIndex", "SetSubtitleStreamIndex"],
            Some(0),
            Some(2),
        ),
        VERSION.into(),
        metadata(tracks),
        VERSION.into(),
        final_session,
    ]
}
#[test]
fn track_selection_sends_exact_general_command_json_once_without_play_or_seek() {
    for (a, name, index) in [
        (audio(1), "SetAudioStreamIndex", "1"),
        (sub(-1), "SetSubtitleStreamIndex", "-1"),
    ] {
        let m = Mock::new(replies(
            session(
                "source",
                &["SetAudioStreamIndex", "SetSubtitleStreamIndex"],
                Some(0),
                Some(2),
            ),
            good_tracks(),
        ));
        apply(&m, &a).unwrap();
        let sent = m.sent.lock().unwrap();
        let posts: Vec<_> = sent.iter().filter(|r| r.mutating).collect();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].path, "/Sessions/session/Command");
        assert_eq!(posts[0].method, "POST");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(posts[0].json_body.as_deref().unwrap())
                .unwrap(),
            serde_json::json!({"Name":name,"Arguments":{"Index":index}})
        );
    }
}
#[test]
fn fresh_missing_caps_or_changed_source_after_metadata_prevents_post() {
    for s in [
        session("source", &[], Some(0), Some(2)),
        session("other", &["SetAudioStreamIndex"], Some(0), Some(2)),
    ] {
        let m = Mock::new(replies(s, good_tracks()));
        assert!(apply(&m, &audio(1)).is_err());
        assert!(m.sent.lock().unwrap().iter().all(|r| !r.mutating));
    }
}
#[test]
fn missing_initial_caps_rejects_even_matching_index_and_metadata_alone_is_insufficient() {
    let m = Mock::new(vec![
        VERSION.into(),
        session("source", &[], Some(1), Some(2)),
    ]);
    assert_eq!(
        apply(&m, &audio(1)).unwrap_err().code,
        "track_selection_unsupported"
    );
    assert!(m.sent.lock().unwrap().iter().all(|r| !r.mutating));
}
#[test]
fn wrong_kind_missing_index_duplicate_or_source_metadata_never_dispatch() {
    for tracks in [
        serde_json::json!([{"Index":1,"Type":"Subtitle"}]),
        serde_json::json!([]),
        serde_json::json!([{"Index":1,"Type":"Audio"},{"Index":1,"Type":"Audio"}]),
    ] {
        let m = Mock::new(replies(
            session("source", &["SetAudioStreamIndex"], Some(0), Some(2)),
            tracks,
        ));
        assert!(apply(&m, &audio(1)).is_err());
        assert!(m.sent.lock().unwrap().iter().all(|r| !r.mutating));
    }
}
#[test]
fn native_null_is_not_subtitle_off_and_noop_uses_fresh_index() {
    let m = Mock::new(replies(
        session("source", &["SetSubtitleStreamIndex"], Some(0), None),
        good_tracks(),
    ));
    apply(&m, &sub(-1)).unwrap();
    assert_eq!(
        m.sent.lock().unwrap().iter().filter(|r| r.mutating).count(),
        1
    );
    let m = Mock::new(replies(
        session("source", &["SetAudioStreamIndex"], Some(1), Some(2)),
        good_tracks(),
    ));
    apply(&m, &audio(1)).unwrap();
    assert!(m.sent.lock().unwrap().iter().all(|r| !r.mutating));
}
#[test]
fn unexpected_post_status_is_uncertain_not_retried() {
    let mut m = Mock::new(replies(
        session("source", &["SetAudioStreamIndex"], Some(0), Some(2)),
        good_tracks(),
    ));
    m.post_status = 500;
    assert_eq!(
        apply(&m, &audio(1)).unwrap_err().effect,
        Effect::MayHaveChanged
    );
    assert_eq!(
        m.sent.lock().unwrap().iter().filter(|r| r.mutating).count(),
        1
    );
}
