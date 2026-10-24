use std::{collections::VecDeque, sync::Mutex};
use tv_adapters::{
    config::Config,
    http::{Http, HttpRequest, HttpResponse},
    jellyfin::Jellyfin,
    runtime::{CancelFlag, SystemClock},
};
use tv_application::{Clock, Context};
use tv_domain::{media::*, PortResult};
struct Mock {
    replies: Mutex<VecDeque<&'static str>>,
    paths: Mutex<Vec<String>>,
}
impl Mock {
    fn new(replies: Vec<&'static str>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            paths: Mutex::new(vec![]),
        }
    }
}
impl Http for Mock {
    fn send(&self, r: HttpRequest, _: &Context<'_>) -> PortResult<HttpResponse> {
        assert!(!r.mutating);
        assert_eq!(r.method, "GET");
        self.paths.lock().unwrap().push(r.path);
        Ok(HttpResponse {
            status: 200,
            body: self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected request")
                .as_bytes()
                .to_vec(),
        })
    }
}
const VERSION: &str = r#"{"Version":"12.0.0"}"#;
fn read(mock: &Mock, read: Read) -> PortResult<Data> {
    let cfg = Config::from_json(include_bytes!("../../../examples/config.json")).unwrap();
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    Jellyfin::new(&cfg, mock).read_media(
        "media_screen",
        &read,
        &Context {
            clock: &clock,
            cancellation: &cancel,
            deadline_monotonic_ms: clock.monotonic_ms() + 1000,
        },
    )
}
#[test]
fn catalog_requires_patched_server_before_private_reads() {
    let mock = Mock::new(vec![r#"{"Version":"10.11.0"}"#]);
    assert_eq!(
        read(
            &mock,
            Read::Search {
                query: "x".into(),
                limit: 1
            }
        )
        .unwrap_err()
        .code,
        "jellyfin_server_requires_security_review"
    );
    assert_eq!(*mock.paths.lock().unwrap(), vec!["/System/Info/Public"]);
}
#[test]
fn search_is_encoded_bounded_and_truncation_is_explicit() {
    let mock = Mock::new(vec![
        VERSION,
        r#"{"Items":[{"Id":"a","Name":"Same","Type":"Movie"},{"Id":"b","Name":"Same","Type":"Movie"}]}"#,
    ]);
    let Data::Items { items, truncated } = read(
        &mock,
        Read::Search {
            query: "A&B".into(),
            limit: 1,
        },
    )
    .unwrap() else {
        panic!()
    };
    assert!(truncated);
    assert_eq!(items.len(), 1);
    let paths = mock.paths.lock().unwrap();
    assert!(paths[1].contains("searchTerm=A%26B"));
    assert!(paths[1].contains("limit=2"));
    assert!(paths[1].contains("userId=example_user"));
}
#[test]
fn item_returns_exact_source_and_track_metadata_without_urls() {
    let mock = Mock::new(vec![
        VERSION,
        r#"{"Items":[{"Id":"episode","Name":"Pilot","Type":"Episode","SeriesId":"series","ParentIndexNumber":0,"IndexNumber":1,"RunTimeTicks":123,"Path":"secret","MediaSources":[{"Id":"source","Path":"secret","MediaStreams":[{"Index":1,"Type":"Audio","Language":"fra","DisplayTitle":"French","IsDefault":true},{"Index":2,"Type":"Subtitle","Language":"eng","IsForced":true}]}]}]}"#,
    ]);
    let Data::Items { items, truncated } = read(
        &mock,
        Read::Item {
            item_id: "episode".into(),
        },
    )
    .unwrap() else {
        panic!()
    };
    assert!(!truncated);
    assert_eq!(items[0].season, Some(0));
    assert_eq!(items[0].sources[0].tracks[1].index, 2);
    assert!(items[0].sources[0].tracks[1].is_forced);
    assert!(!format!("{items:?}").contains("secret"));
}
#[test]
fn item_identity_is_not_inferred_from_title() {
    let mock = Mock::new(vec![
        VERSION,
        r#"{"Items":[{"Id":"other","Name":"Same","Type":"Movie"}]}"#,
    ]);
    assert_eq!(
        read(
            &mock,
            Read::Item {
                item_id: "wanted".into()
            }
        )
        .unwrap_err()
        .code,
        "jellyfin_item_identity_mismatch"
    );
}
#[test]
fn episode_requires_series_and_season_identity() {
    let mock = Mock::new(vec![
        VERSION,
        r#"{"Items":[{"Id":"episode","Name":"Pilot","Type":"Episode","SeriesId":"other","ParentIndexNumber":1}]}"#,
    ]);
    assert_eq!(
        read(
            &mock,
            Read::Episodes {
                series_id: "series".into(),
                season: Some(1),
                limit: 5
            }
        )
        .unwrap_err()
        .code,
        "jellyfin_episode_identity_mismatch"
    );
}
#[test]
fn excessive_or_duplicate_items_fail_closed() {
    for (body, code) in [
        (
            r#"{"Items":[{"Id":"a","Name":"A","Type":"Movie"},{"Id":"a","Name":"A","Type":"Movie"}]}"#,
            "jellyfin_duplicate_media",
        ),
        (
            r#"{"Items":[{"Id":"a","Name":"A","Type":"Movie"},{"Id":"b","Name":"B","Type":"Movie"},{"Id":"c","Name":"C","Type":"Movie"}]}"#,
            "jellyfin_catalogue_limit",
        ),
    ] {
        let mock = Mock::new(vec![VERSION, body]);
        assert_eq!(
            read(
                &mock,
                Read::Search {
                    query: "q".into(),
                    limit: 1
                }
            )
            .unwrap_err()
            .code,
            code
        );
    }
}
#[test]
fn playback_preserves_unknown_capabilities_and_selected_indices() {
    let mock = Mock::new(vec![
        VERSION,
        r#"[{"Id":"session","DeviceId":"example_device","UserId":"example_user","SupportsRemoteControl":true,"Client":"Android TV","ApplicationVersion":"0.18","SupportedCommands":["SetSubtitleStreamIndex"],"NowPlayingItem":{"Id":"episode"},"PlayState":{"IsPaused":true,"PositionTicks":null,"CanSeek":false,"AudioStreamIndex":1,"SubtitleStreamIndex":-1,"MediaSourceId":"source"}}]"#,
    ]);
    let Data::Playback(p) = read(&mock, Read::Playback).unwrap() else {
        panic!()
    };
    assert_eq!(p.position_ticks, None);
    assert_eq!(p.can_seek, Some(false));
    assert_eq!(p.subtitle_index, Some(-1));
    assert_eq!(p.source_id.as_deref(), Some("source"));
    assert_eq!(p.supported_commands, vec!["SetSubtitleStreamIndex"]);
}
#[test]
fn missing_items_and_ambiguous_sessions_are_not_observed_successfully() {
    let mock = Mock::new(vec![VERSION, r#"{"Items":[]}"#]);
    assert_eq!(
        read(
            &mock,
            Read::Item {
                item_id: "missing".into()
            }
        )
        .unwrap_err()
        .code,
        "jellyfin_item_not_found"
    );
    let mock = Mock::new(vec![VERSION, r#"[]"#]);
    assert_eq!(
        read(&mock, Read::Playback).unwrap_err().code,
        "jellyfin_session_not_found"
    );
}
