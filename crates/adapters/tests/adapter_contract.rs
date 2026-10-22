use std::{collections::VecDeque, sync::Mutex};
use tv_adapters::{
    adb::{parse_playback, parse_volume},
    config::Config,
    http::{curl_config, Http, HttpRequest, HttpResponse},
    jellyfin::{patched_version, Jellyfin},
    runtime::{CancelFlag, SystemClock},
};
use tv_application::{Clock, Context};
use tv_domain::*;
fn config() -> Config {
    Config::from_json(include_bytes!("../../../examples/config.json")).unwrap()
}
fn context<'a>(clock: &'a SystemClock, flag: &'a CancelFlag) -> Context<'a> {
    Context {
        clock,
        cancellation: flag,
        deadline_monotonic_ms: clock.monotonic_ms() + 1000,
    }
}
struct HttpMock {
    replies: Mutex<VecDeque<(u16, &'static str)>>,
    sent: Mutex<Vec<HttpRequest>>,
}
impl HttpMock {
    fn new(replies: Vec<(u16, &'static str)>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            sent: Mutex::new(Vec::new()),
        }
    }
}
impl Http for HttpMock {
    fn send(&self, request: HttpRequest, _: &Context<'_>) -> PortResult<HttpResponse> {
        self.sent.lock().unwrap().push(request);
        let (status, body) = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected request");
        Ok(HttpResponse {
            status,
            body: body.as_bytes().to_vec(),
        })
    }
}
const VERSION: &str = r#"{"Version":"12.0.0"}"#;
const SESSION: &str = r#"[{"Id":"session","DeviceId":"example_device","UserId":"example_user","SupportsRemoteControl":true,"NowPlayingItem":{"Id":"episode"},"PlayState":{"IsPaused":false,"PositionTicks":123}}]"#;
#[test]
fn jellyfin_resolves_resume_under_configured_series() {
    let cfg = config();
    let http = HttpMock::new(vec![(
        200,
        r#"{"Items":[{"Id":"episode","SeriesId":"example_series","UserData":{"PlaybackPositionTicks":123}}]}"#,
    )]);
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    let plan = Jellyfin::new(&cfg, &http)
        .resolve("media_screen", "series", &context(&clock, &flag))
        .unwrap();
    assert_eq!(plan.position_ticks, 123);
    let sent = http.sent.lock().unwrap();
    assert!(sent[0].path.contains("parentId=example_series"));
    assert!(!sent[0].mutating);
}
#[test]
fn jellyfin_rejects_other_series() {
    let cfg = config();
    let http = HttpMock::new(vec![(
        200,
        r#"{"Items":[{"Id":"episode","SeriesId":"other","UserData":{"PlaybackPositionTicks":123}}]}"#,
    )]);
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    assert_eq!(
        Jellyfin::new(&cfg, &http)
            .resolve("media_screen", "series", &context(&clock, &flag))
            .unwrap_err()
            .code,
        "resume_media_identity_mismatch"
    );
}
#[test]
fn jellyfin_204_only_means_dispatch() {
    let cfg = config();
    let http = HttpMock::new(vec![(200, VERSION), (200, SESSION), (204, "")]);
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    Jellyfin::new(&cfg, &http)
        .apply(
            "media_screen",
            &Action::ResumeMedia {
                media: "series".into(),
            },
            Some(&MediaPlan {
                item_id: "episode".into(),
                position_ticks: 123,
            }),
            &context(&clock, &flag),
        )
        .unwrap();
    let sent = http.sent.lock().unwrap();
    assert_eq!(sent[2].method, "POST");
    assert!(sent[2].path.contains("startPositionTicks=123"));
    assert!(sent[2].mutating);
}
#[test]
fn jellyfin_ambiguous_command_response_not_retried() {
    let cfg = config();
    let http = HttpMock::new(vec![(200, VERSION), (200, SESSION), (500, "")]);
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    let e = Jellyfin::new(&cfg, &http)
        .apply(
            "media_screen",
            &Action::Pause,
            None,
            &context(&clock, &flag),
        )
        .unwrap_err();
    assert_eq!(e.effect, Effect::MayHaveChanged);
    assert_eq!(http.sent.lock().unwrap().len(), 3);
}
#[test]
fn jellyfin_refuses_unpatched_and_prerelease_servers() {
    for version in ["10.11.0", "12.0-RC5", "12.0.0-beta", "invalid"] {
        assert!(!patched_version(version));
    }
    assert!(patched_version("12.0"));
    assert!(patched_version("12.1.0"));
    let cfg = config();
    let http = HttpMock::new(vec![(200, r#"{"Version":"12.0-RC5"}"#)]);
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    assert_eq!(
        Jellyfin::new(&cfg, &http)
            .observe("media_screen", &context(&clock, &flag))
            .unwrap_err()
            .code,
        "jellyfin_server_requires_security_review"
    );
    assert_eq!(http.sent.lock().unwrap().len(), 1);
}
#[test]
fn jellyfin_refuses_wrong_user_even_when_server_returns_session() {
    let cfg = config();
    let http = HttpMock::new(vec![
        (200, VERSION),
        (
            200,
            r#"[{"Id":"session","DeviceId":"example_device","UserId":"other","SupportsRemoteControl":true}]"#,
        ),
    ]);
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    assert_eq!(
        Jellyfin::new(&cfg, &http)
            .observe("media_screen", &context(&clock, &flag))
            .unwrap_err()
            .code,
        "jellyfin_session_not_found"
    );
}
#[test]
fn curl_secret_is_stdin_configuration_and_redirects_not_enabled() {
    let cfg = config();
    let input = curl_config(
        &cfg.servers["library"],
        "GET",
        "/Sessions",
        "fixture_only",
        500,
    )
    .unwrap();
    assert!(input.contains("X-Emby-Token: fixture_only"));
    assert!(!input.contains("location"));
    assert!(!input.contains("insecure\n"));
    assert!(curl_config(
        &cfg.servers["library"],
        "GET",
        "/Sessions",
        "bad\nheader",
        500
    )
    .is_err());
}
#[test]
fn adb_parses_only_target_session_and_omits_metadata() {
    let raw="package=org.other.player\nstate=PlaybackState {state=3, speed=1.0}\npackage=org.example.player\nmetadata=private title\nstate=PlaybackState {state=2, speed=0.0}";
    assert_eq!(
        parse_playback(raw, "org.example.player").unwrap(),
        Playback::Paused
    );
}
#[test]
fn adb_rejects_ambiguous_sessions() {
    let raw="package=org.example.player\nstate=PlaybackState {state=2}\npackage=org.example.player\nstate=PlaybackState {state=3}";
    assert_eq!(
        parse_playback(raw, "org.example.player").unwrap_err().code,
        "media_session_ambiguous"
    );
}
#[test]
fn volume_parser_does_not_confuse_index_and_percent() {
    assert_eq!(
        parse_volume("[v] volume is 4 in range [0..15]"),
        Some((4, 15))
    );
    assert_eq!(parse_volume("volume is 40 in range [0..15]"), None);
    assert_eq!(parse_volume("unavailable"), None);
}
#[test]
fn configuration_rejects_unknown_fields_and_bad_references() {
    let mut value: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../examples/config.json")).unwrap();
    value["unexpected"] = true.into();
    assert!(Config::from_json(value.to_string().as_bytes()).is_err());
    value.as_object_mut().unwrap().remove("unexpected");
    value["devices"]["screen"]["media_app"] = "unknown".into();
    assert!(Config::from_json(value.to_string().as_bytes()).is_err());
}
#[test]
fn configuration_rejects_implicit_cleartext_credentials() {
    let mut cfg = config();
    cfg.servers.get_mut("library").unwrap().base_url = "http://media.example.invalid".into();
    assert_eq!(cfg.validate(), Err("tls_required"));
}

#[test]
fn configuration_rejects_duplicate_aliases_shell_payloads_and_url_globs() {
    let raw = include_str!("../../../examples/config.json");
    let duplicate = raw.replace(
        "\"apps\": {",
        "\"apps\": {\"player\":{\"package\":\"org.example.other\"},",
    );
    assert!(Config::from_json(duplicate.as_bytes()).is_err());
    let mut cfg = config();
    cfg.apps.get_mut("player").unwrap().package = "org.example.player;reboot".into();
    assert_eq!(cfg.validate(), Err("invalid_package"));
    let mut cfg = config();
    cfg.servers.get_mut("library").unwrap().base_url =
        "https://{first,second}.example.invalid".into();
    assert_eq!(cfg.validate(), Err("invalid_server_url"));
}

#[test]
fn foreground_requires_exact_package_and_unique_resumed_activity() {
    use tv_adapters::adb::parse_foreground;
    assert_eq!(
        parse_foreground(
            "mResumedActivity: ActivityRecord{a u0 org.bad.org.example.player/.Main t1}"
        )
        .unwrap(),
        Some("org.bad.org.example.player".into())
    );
    assert_eq!(parse_foreground("mResumedActivity: ActivityRecord{a u0 org.example.player/.Main t1}\ntopResumedActivity: ActivityRecord{b u0 org.other.player/.Main t2}").unwrap_err().code,"foreground_ambiguous");
}
