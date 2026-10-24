//! Small wire boundary for the generic TV contract. No platform logic lives here.
use serde::Deserialize;
use serde_json::{json, Value};
use tv_application::{generic::Control, Cancellation};
use tv_domain::{generic::*, Observed, Playback, Status};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mutation {
    request_id: String,
    device: String,
    expires_at_unix_ms: u64,
    action: Action,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    OpenApp {
        app: String,
    },
    OpenUrl {
        app: String,
        url: String,
    },
    Wake {},
    Standby {},
    Mute {},
    SetVolume {
        index: u16,
    },
    Pause {},
    Resume {},
    Media {
        session: String,
        command: WireMediaCommand,
    },
    Repeat {
        direction: WireDirection,
        count: u8,
    },
    LongSelect {},
    Navigate {
        keys: Vec<WireKey>,
    },
    Search {
        app: String,
        query: String,
    },
    Activate {
        reference: String,
    },
    TypeText {
        reference: String,
        text: String,
    },
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireMediaCommand {
    Pause,
    Resume,
    Next,
    Previous,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireKey {
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
    Home,
    Menu,
    Settings,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireDirection {
    Up,
    Down,
    Left,
    Right,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reading {
    device: String,
    expires_at_unix_ms: u64,
    read: ReadKind,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ReadKind {
    AppDetails {
        app: String,
    },
    Media {},
    State {},
    RemoteCapabilities {},
    Volume {},
    Power {},
    Apps {
        query: Option<String>,
        since: Option<String>,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        refresh: bool,
    },
    Ui {},
    Screenshot {},
}
fn default_limit() -> usize {
    12
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationRequest {
    request_id: String,
    #[serde(default)]
    cancel: bool,
}

pub fn routes_generic(config: &tv_adapters::config::Config, name: &str, args: &Value) -> bool {
    if matches!(name, "tv_read" | "tv_operation") {
        return true;
    }
    if name != "tv_control" {
        return false;
    }
    let action = args.pointer("/action/type").and_then(Value::as_str);
    if matches!(
        action,
        Some(
            "mute"
                | "wake"
                | "standby"
                | "navigate"
                | "search"
                | "open_url"
                | "activate"
                | "media"
                | "type_text"
                | "repeat"
                | "long_select"
        )
    ) {
        return true;
    }
    let device = args
        .get("device")
        .and_then(Value::as_str)
        .and_then(|d| config.devices.get(d));
    let Some(tv_adapters::config::Device::Adb { media_app, .. }) = device else {
        return false;
    };
    let legacy = match action {
        Some("open_app") => args
            .pointer("/action/app")
            .and_then(Value::as_str)
            .is_some_and(|app| config.apps.contains_key(app)),
        Some("pause" | "resume") => media_app.is_some(),
        Some("set_volume") => return config.generic_adb_control,
        _ => return false,
    };
    config.generic_adb_control || !legacy
}
pub struct Response {
    pub value: Value,
    pub failed: bool,
    pub image: Option<Vec<u8>>,
}
/// Preserve explicitly configured aliases after upgrading to discovered apps.
/// The generic resolver still verifies installation and launchable components.
pub fn translate_alias(config: &tv_adapters::config::Config, args: &mut Value) {
    if matches!(
        args.pointer("/action/type").and_then(Value::as_str),
        Some("open_app" | "open_url" | "search")
    ) {
        if let Some(package) = args
            .pointer("/action/app")
            .and_then(Value::as_str)
            .and_then(|name| config.apps.get(name))
            .map(|app| app.package.clone())
        {
            args["action"]["app"] = json!(package);
        }
    }
}
pub enum Call {
    Read(ReadRequest),
    Execute(Request),
    Operation { id: String, cancel: bool },
}
impl Call {
    pub fn parse(name: &str, args: Value) -> Result<Self, &'static str> {
        let invalid = |_| "invalid_arguments";
        match name {
            "tv_read" => {
                let r: Reading = serde_json::from_value(args).map_err(invalid)?;
                Ok(Self::Read(ReadRequest {
                    device: r.device,
                    expires_at_unix_ms: r.expires_at_unix_ms,
                    read: match r.read {
                        ReadKind::Media {} => Read::Media,
                        ReadKind::State {} => Read::State,
                        ReadKind::RemoteCapabilities {} => Read::RemoteCapabilities,
                        ReadKind::Volume {} => Read::Volume,
                        ReadKind::Power {} => Read::Power,
                        ReadKind::Apps {
                            query,
                            since,
                            limit,
                            refresh,
                        } => Read::Apps {
                            query,
                            since,
                            limit,
                            refresh,
                        },
                        ReadKind::AppDetails { app } => Read::AppDetails { app },
                        ReadKind::Ui {} => Read::Ui,
                        ReadKind::Screenshot {} => Read::Screenshot,
                    },
                }))
            }
            "tv_operation" => {
                let r: OperationRequest = serde_json::from_value(args).map_err(invalid)?;
                if !tv_domain::identifier(&r.request_id) {
                    return Err("invalid_arguments");
                }
                Ok(Self::Operation {
                    id: r.request_id,
                    cancel: r.cancel,
                })
            }
            "tv_control" => {
                let r: Mutation = serde_json::from_value(args).map_err(invalid)?;
                Ok(Self::Execute(Request {
                    id: r.request_id,
                    device: r.device,
                    expires_at_unix_ms: r.expires_at_unix_ms,
                    intent: match r.action {
                        Action::OpenApp { app } => Intent::OpenApp { app },
                        Action::OpenUrl { app, url } => Intent::OpenUrl { app, url },
                        Action::Wake {} => Intent::Wake,
                        Action::Standby {} => Intent::Standby,
                        Action::Mute {} => Intent::Mute,
                        Action::SetVolume { index } => Intent::SetVolume { index },
                        Action::Pause {} => Intent::Pause,
                        Action::Resume {} => Intent::Resume,
                        Action::Media { session, command } => Intent::Media {
                            session,
                            command: match command {
                                WireMediaCommand::Pause => MediaCommand::Pause,
                                WireMediaCommand::Resume => MediaCommand::Resume,
                                WireMediaCommand::Next => MediaCommand::Next,
                                WireMediaCommand::Previous => MediaCommand::Previous,
                            },
                        },
                        Action::LongSelect {} => Intent::LongSelect,
                        Action::Repeat { direction, count } => {
                            if !(1..=16).contains(&count) {
                                return Err("invalid_arguments");
                            }
                            let key = match direction {
                                WireDirection::Up => Key::Up,
                                WireDirection::Down => Key::Down,
                                WireDirection::Left => Key::Left,
                                WireDirection::Right => Key::Right,
                            };
                            Intent::Navigate {
                                keys: vec![key; usize::from(count)],
                            }
                        }
                        Action::Navigate { keys } => Intent::Navigate {
                            keys: keys
                                .into_iter()
                                .map(|k| match k {
                                    WireKey::Up => Key::Up,
                                    WireKey::Down => Key::Down,
                                    WireKey::Left => Key::Left,
                                    WireKey::Right => Key::Right,
                                    WireKey::Select => Key::Select,
                                    WireKey::Back => Key::Back,
                                    WireKey::Home => Key::Home,
                                    WireKey::Menu => Key::Menu,
                                    WireKey::Settings => Key::Settings,
                                })
                                .collect(),
                        },
                        Action::Search { app, query } => Intent::Search { app, query },
                        Action::Activate { reference } => Intent::Activate { reference },
                        Action::TypeText { reference, text } => {
                            Intent::TypeText { reference, text }
                        }
                    },
                }))
            }
            _ => Err("unknown_tool"),
        }
    }
    pub fn expires(&self) -> Option<u64> {
        match self {
            Self::Read(r) => Some(r.expires_at_unix_ms),
            Self::Execute(r) => Some(r.expires_at_unix_ms),
            Self::Operation { .. } => None,
        }
    }
    pub fn execute(self, control: &dyn Control, cancel: &dyn Cancellation) -> Response {
        let (value, failed) = match self {
            Self::Execute(r) => {
                let r = control.execute(r, cancel);
                let failed = !matches!(r.status, Status::Observed | Status::Verified);
                (reply(&r), failed)
            }
            Self::Read(r) => match control.read(r, cancel) {
                Ok(data) => (
                    match data {
                        Data::RemoteCapabilities(c) => {
                            json!({"android_sdk":c.android_sdk,"app":c.active_app,"keyevent_help":c.keyevent_help,"longpress_help":c.longpress_help,"standby_key_supported":c.standby_key_supported,"standby_effect":"device_dependent","app_key_acceptance":"unknown","evidence":"device_input_help"})
                        }
                        Data::AppDetails(details) => app_details(&details),
                        Data::State(s) => json!({"state": state(&s)}),
                        Data::Media {
                            sessions,
                            truncated,
                        } => {
                            let mut value =
                                json!({"sessions": sessions.iter().map(media).collect::<Vec<_>>()});
                            if truncated {
                                value["truncated"] = json!(true);
                            }
                            value
                        }
                        Data::Screenshot(s) => {
                            return Response {
                                value: json!({"width":s.width,"height":s.height,"format":"png"}),
                                failed: false,
                                image: Some(s.png),
                            }
                        }
                        Data::Apps {
                            revision,
                            unchanged,
                            apps,
                            truncated,
                        } => {
                            let mut value = json!({"revision":revision});
                            if unchanged {
                                value["unchanged"] = json!(true);
                            } else {
                                value["apps"] = apps.iter().map(app).collect::<Vec<_>>().into();
                                if truncated {
                                    value["truncated"] = json!(true);
                                }
                            }
                            value
                        }
                        Data::Ui(ui) => {
                            json!({"revision":ui.revision,"truncation":ui.truncation.iter().map(|reason| match reason { UiTruncation::Elements=>"elements", UiTruncation::Text=>"text" }).collect::<Vec<_>>(),"focused_count":ui.focused_count,"elements":ui.elements.iter().map(ui_element).collect::<Vec<_>>()})
                        }
                    },
                    false,
                ),
                Err(e) => (json!({"code":e.code}), true),
            },
            Self::Operation { id, cancel } => match control.operation(&id, cancel) {
                Ok(op) => (
                    match op {
                        Operation::Missing => json!({"request_id":id,"status":"not_found"}),
                        Operation::Running => {
                            json!({"request_id":id,"status":"running","cancel_requested":cancel})
                        }
                        Operation::Complete(r) => reply(&r),
                    },
                    false,
                ),
                Err(e) => (json!({"code":e.code}), true),
            },
        };
        Response {
            value,
            failed,
            image: None,
        }
    }
}
fn ui_element(e: &UiElement) -> Value {
    let mut value = json!({"ref":e.reference,"text":e.text,"role":e.role,"focused":e.focused,"actionable":e.actionable,"editable":e.editable});
    if let Some(text) = &e.accessibility_text {
        value["accessibility_text"] = json!(text);
    }
    value
}
fn media(s: &MediaSession) -> Value {
    let mut v = json!({"session":s.session,"package":s.package,"playback": playback(s.playback)});
    if let Some(commands) = &s.supported_commands {
        v["supported_commands"] = json!(commands
            .iter()
            .map(|command| match command {
                MediaCommand::Pause => "pause",
                MediaCommand::Resume => "resume",
                MediaCommand::Next => "next",
                MediaCommand::Previous => "previous",
            })
            .collect::<Vec<_>>());
    }
    if let Some(position) = s.position_ms {
        v["position_ms"] = json!(position);
    }
    if let Some(description) = &s.description {
        v["description"] = json!(description);
    }
    v
}
fn playback(p: Playback) -> &'static str {
    match p {
        Playback::Playing => "playing",
        Playback::Paused => "paused",
        Playback::Stopped => "stopped",
        Playback::Buffering => "buffering",
        Playback::Unknown => "unknown",
    }
}
fn app_details(details: &AppDetails) -> Value {
    let search = match &details.native_search {
        NativeSearch::Handler { component } => {
            json!({"status":"handler_resolved","component":component})
        }
        NativeSearch::Unsupported => json!({"status":"unsupported"}),
        NativeSearch::Unknown => json!({"status":"unknown"}),
    };
    let version = match &details.version {
        Some(version) => {
            json!({"status":"observed","code":version.code,"name":version.name,"evidence":"active_package_dump"})
        }
        None => json!({"status":"unknown"}),
    };
    json!({"id":details.app.id,"component":details.app.component,"version":version,
        "native_search":search,"runtime_acceptance":"unknown","url_support":"requires_specific_uri_resolution"})
}
fn app(a: &App) -> Value {
    let mut v = json!({"id":a.id,"component":a.component});
    if let Some(label) = &a.label {
        v["label"] = json!(label);
    }
    if a.search_component.is_some() {
        v["search"] = json!(true);
    }
    v
}
fn state(s: &Observed) -> Value {
    let mut v = json!({"connected":s.connected});
    if let Some(app) = &s.active_app {
        v["app"] = json!(app);
    }
    if s.playback != Playback::Unknown {
        v["playback"] = json!(match s.playback {
            Playback::Playing => "playing",
            Playback::Paused => "paused",
            Playback::Stopped => "stopped",
            Playback::Buffering => "buffering",
            Playback::Unknown => "unknown",
        });
    }
    if let Some(item) = &s.media_id {
        v["media_id"] = json!(item);
    }
    if let Some(source) = &s.media_source_id {
        v["media_source_id"] = json!(source);
    }
    if let Some(position) = s.position_ticks {
        v["position_ticks"] = json!(position);
    }
    if let Some(awake) = s.awake {
        v["awake"] = json!(awake);
        v["power_scope"] = json!("android_device_not_physical_display");
    }
    if let Some(index) = s.volume_index {
        v["volume_index"] = json!(index);
    }
    if let Some(max) = s.volume_max_index {
        v["volume_max_index"] = json!(max);
    }
    v
}
fn reply(r: &Reply) -> Value {
    let mut v = json!({"request_id":r.request_id,"status":match r.status { Status::Observed=>"observed",Status::Verified=>"verified",Status::Invalid=>"invalid",Status::Busy=>"busy",Status::Unsupported=>"unsupported",Status::Expired=>"expired",Status::Cancelled=>"cancelled",Status::Failed=>"failed",Status::OutcomeUnknown=>"outcome_unknown" },"code":r.code,"local_elapsed_ms":r.local_elapsed_ms,"timing_scope":"application_execution"});
    if r.cached {
        v["cached"] = json!(true);
    }
    if let Some(s) = &r.observed {
        v["state"] = state(s);
        v["state_scope"] = json!("execution_observation");
    }
    if !r.choices.is_empty() {
        v["choices"] = r.choices.iter().map(app).collect::<Vec<_>>().into();
    }
    v
}

/// Extend the legacy schema without app-specific tools or unbounded catalogue fields.
pub fn extend_tools(tools: &mut Value) {
    let list = tools["tools"].as_array_mut().expect("tool schema");
    list.iter_mut()
        .find(|t| t["name"] == "tv_capabilities")
        .expect("capabilities tool")["description"] = json!("Capabilities; no device access.");
    let control = list
        .iter_mut()
        .find(|t| t["name"] == "tv_control")
        .expect("control tool");
    control["description"] = json!("Never retry outcome_unknown. Native volume index, not %. Mute: stream 3 zero, not speaker proof. Media: session from read media. Search/URL/next/previous: dispatch only. Text: focused editable ref; ASCII except %/?, matching accessibility readback; mismatch unknown, no repair. Unique IDs.");
    let actions = control["inputSchema"]["properties"]["action"]["oneOf"]
        .as_array_mut()
        .unwrap();
    for action in actions.iter_mut() {
        if action["properties"]["type"]["const"] == "open_app" {
            action["properties"]["app"]["maxLength"] = json!(512);
        }
    }
    actions.extend([
        json!({"type":"object","properties":{"type":{"const":"media"},"session":{"type":"string","maxLength":384},"command":{"enum":["pause","resume","next","previous"]}},"required":["type","session","command"],"additionalProperties":false}),
        json!({"type":"object","properties":{"type":{"const":"type_text"},"reference":{"type":"string","minLength":1,"maxLength":200},"text":{"type":"string","minLength":1,"maxLength":500}},"required":["type","reference","text"],"additionalProperties":false}),
        json!({"type":"object","properties":{"type":{"const":"repeat"},"direction":{"enum":["up","down","left","right"]},"count":{"type":"integer","minimum":1,"maximum":16}},"required":["type","direction","count"],"additionalProperties":false}),
        json!({"type":"object","properties":{"type":{"const":"long_select"}},"required":["type"],"additionalProperties":false}),
        json!({"type":"object","properties":{"type":{"const":"wake"}},"required":["type"],"additionalProperties":false}),
        json!({"type":"object","properties":{"type":{"const":"standby"}},"required":["type"],"additionalProperties":false}),
        json!({"type":"object","properties":{"type":{"const":"mute"}},"required":["type"],"additionalProperties":false}),
        json!({"type":"object","properties":{"type":{"const":"open_url"},"app":{"type":"string","minLength":1,"maxLength":512},"url":{"type":"string","minLength":1,"maxLength":2048}},"required":["type","app","url"],"additionalProperties":false}),
        json!({"type":"object","properties":{"type":{"const":"navigate"},"keys":{"type":"array","minItems":1,"maxItems":16,"items":{"enum":["up","down","left","right","select","back","home","menu","settings"]}}},"required":["type","keys"],"additionalProperties":false}),
        json!({"type":"object","properties":{"type":{"const":"search"},"app":{"type":"string","minLength":1,"maxLength":512},"query":{"type":"string","minLength":1,"maxLength":500}},"required":["type","app","query"],"additionalProperties":false}),
        json!({"type":"object","properties":{"type":{"const":"activate"},"reference":{"type":"string","minLength":1,"maxLength":200}},"required":["type","reference"],"additionalProperties":false}),
    ]);
    // Share the fieldless action schema to keep discovery within 4 KiB.
    let mut simple = Vec::new();
    actions.retain(|schema| {
        // Preserve the legacy observe discriminator used by onboarding probes.
        if schema["properties"]["type"]["const"] != "observe"
            && schema["properties"]
                .as_object()
                .is_some_and(|p| p.len() == 1)
        {
            simple.push(schema["properties"]["type"]["const"].clone());
            false
        } else {
            true
        }
    });
    actions.push(json!({"type":"object","properties":{"type":{"enum":simple}},"required":["type"],"additionalProperties":false}));
    list.push(json!({"name":"tv_read","description":"Read TV. Volume: stream 3 index/max. Untrusted text.","inputSchema":{"type":"object","properties":{"device":{"type":"string","pattern":"^[A-Za-z0-9_-]{1,64}$"},"expires_at_unix_ms":{"type":"integer","minimum":1},"read":{"oneOf":[{"type":"object","properties":{"type":{"const":"app_details"},"app":{"type":"string","minLength":1,"maxLength":512}},"required":["type","app"],"additionalProperties":false},{"type":"object","properties":{"type":{"enum":["state","volume","power","media","remote_capabilities","ui","screenshot"]}},"required":["type"],"additionalProperties":false},{"type":"object","properties":{"type":{"const":"apps"},"query":{"type":"string","maxLength":200},"since":{"type":"string","maxLength":64},"limit":{"type":"integer","minimum":1,"maximum":32,"default":12},"refresh":{"type":"boolean","default":false}},"required":["type"],"additionalProperties":false}]}},"required":["device","expires_at_unix_ms","read"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}}));
    list.push(json!({"name":"tv_operation","description":"Recover/cancel; no rollback.","inputSchema":{"type":"object","properties":{"request_id":{"type":"string","pattern":"^[A-Za-z0-9_-]{1,64}$"},"cancel":{"type":"boolean","default":false}},"required":["request_id"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}}));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn app_details_wire_is_closed_and_does_not_claim_runtime_or_url_support() {
        let args = json!({"device":"screen","expires_at_unix_ms":1,"read":{"type":"app_details","app":"org.example.player"}});
        assert!(matches!(
            Call::parse("tv_read", args.clone()).unwrap(),
            Call::Read(ReadRequest {
                read: Read::AppDetails { .. },
                ..
            })
        ));
        let mut extra = args;
        extra["read"]["url"] = json!("https://example.org");
        assert!(Call::parse("tv_read", extra).is_err());
        let details = AppDetails {
            app: App {
                id: "org.example.player".into(),
                component: "org.example.player/.Main".into(),
                label: None,
                search_component: Some("org.example.player/.Search".into()),
            },
            version: None,
            native_search: NativeSearch::Unsupported,
        };
        let value = app_details(&details);
        assert_eq!(value["id"], "org.example.player");
        assert_eq!(value["version"]["status"], "unknown");
        assert_eq!(value["native_search"]["status"], "unsupported");
        assert_eq!(value["runtime_acceptance"], "unknown");
        assert_eq!(value["url_support"], "requires_specific_uri_resolution");
        assert!(value.get("search").is_none());
        assert!(value.to_string().len() < 1024);
    }
    #[test]
    fn media_wire_is_typed_and_rejects_unsupported_transport_commands() {
        for command in ["pause", "resume", "next", "previous"] {
            assert!(Call::parse("tv_control", json!({"request_id":"r","device":"screen","expires_at_unix_ms":1,"action":{"type":"media","session":"org.example.player/player","command":command}})).is_ok());
        }
        for command in ["stop", "seek", "play-pause", "reboot"] {
            assert!(Call::parse("tv_control", json!({"request_id":"r","device":"screen","expires_at_unix_ms":1,"action":{"type":"media","session":"org.example.player/player","command":command}})).is_err());
        }
        assert!(Call::parse(
            "tv_read",
            json!({"device":"screen","expires_at_unix_ms":1,"read":{"type":"media"}})
        )
        .is_ok());
        assert!(Call::parse("tv_control", json!({"request_id":"r","device":"screen","expires_at_unix_ms":1,"action":{"type":"media","session":"org.example.player/player","command":"next","position_ms":10}})).is_err());
    }
    #[test]
    fn remote_wire_is_closed_and_repeat_expands_to_bounded_navigation() {
        for action in [
            json!({"type":"repeat","direction":"down","count":0}),
            json!({"type":"repeat","direction":"down","count":17}),
            json!({"type":"repeat","direction":"select","count":2}),
            json!({"type":"repeat","direction":"down","count":256}),
            json!({"type":"long_select","duration_ms":1}),
            json!({"type":"navigate","keys":["power"]}),
            json!({"type":"navigate","keys":["volume_up"]}),
        ] {
            assert!(Call::parse("tv_control", json!({"request_id":"r","device":"screen","expires_at_unix_ms":2000,"action":action})).is_err());
        }
        let call = Call::parse("tv_control", json!({"request_id":"r","device":"screen","expires_at_unix_ms":2000,"action":{"type":"repeat","direction":"left","count":16}})).unwrap();
        assert!(
            matches!(call, Call::Execute(Request { intent: Intent::Navigate { keys }, .. }) if keys == vec![Key::Left;16])
        );
    }
    #[test]
    fn wire_rejects_unknown_actions_and_fields() {
        assert!(Call::parse("tv_control", json!({"request_id":"r","device":"screen","expires_at_unix_ms":1,"action":{"type":"standby","toggle":true}})).is_err());
        assert!(Call::parse("tv_control", json!({"request_id":"r","device":"screen","expires_at_unix_ms":1,"action":{"type":"legacy","action":"anything"}})).is_err());
        assert!(Call::parse("tv_operation", json!({"request_id":"r","shell":"x"})).is_err());
        assert!(Call::parse(
            "tv_read",
            json!({"device":"screen","expires_at_unix_ms":1,"read":{"type":"state","query":"x"}})
        )
        .is_err());
    }
    #[test]
    fn individual_schemas_fit_minimum_paginated_transport_budget() {
        let mut tools = crate::mcp::tools();
        extend_tools(&mut tools);
        let control = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "tv_control")
            .unwrap();
        assert!(
            control["inputSchema"]["properties"]["action"]["oneOf"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action["properties"]["type"]["const"] == "observe"),
            "onboarding probe requires the legacy observe discriminator"
        );
        crate::media_mcp::extend_tools(&mut tools);
        // The catalog may grow across pages; every individual tool must still
        // fit the minimum configured frame, including cursor/newline overhead.
        for tool in tools["tools"].as_array().unwrap() {
            let frame =
                json!({"jsonrpc":"2.0","id":1,"result":{"tools":[tool],"nextCursor":"999"}});
            assert!(
                frame.to_string().len() < 4096,
                "tool schema too large: {}",
                tool["name"]
            );
        }
    }
    #[test]
    fn legacy_aliases_keep_their_scope_until_generic_permission_is_granted() {
        let mut cfg =
            tv_adapters::config::Config::from_json(include_bytes!("../../../examples/config.json"))
                .unwrap();
        let alias = cfg.apps.keys().next().unwrap().clone();
        let known = json!({"device":"screen","action":{"type":"open_app","app":alias}});
        assert!(!routes_generic(&cfg, "tv_control", &known));
        assert!(routes_generic(
            &cfg,
            "tv_control",
            &json!({"device":"screen","action":{"type":"open_app","app":"Unconfigured app"}})
        ));
        cfg.generic_adb_control = true;
        assert!(routes_generic(&cfg, "tv_control", &known));
        assert!(routes_generic(
            &cfg,
            "tv_control",
            &json!({"device":"screen","action":{"type":"set_volume","index":1}})
        ));
        assert!(!routes_generic(&cfg, "tv_verify_connection", &known));
    }
    #[test]
    fn upgraded_aliases_resolve_to_their_configured_package_without_bypassing_discovery() {
        let cfg =
            tv_adapters::config::Config::from_json(include_bytes!("../../../examples/config.json"))
                .unwrap();
        let (name, app) = cfg.apps.iter().next().unwrap();
        let mut args = json!({"request_id":"r","device":"screen","expires_at_unix_ms":1,"action":{"type":"open_app","app":name}});
        translate_alias(&cfg, &mut args);
        assert_eq!(args["action"]["app"], app.package);
        assert!(matches!(
            Call::parse("tv_control", args).unwrap(),
            Call::Execute(Request {
                intent: Intent::OpenApp { .. },
                ..
            })
        ));
    }
    #[test]
    fn successful_results_are_compact() {
        let r = Reply {
            request_id: "r".into(),
            status: Status::Verified,
            code: "state_verified".into(),
            observed: Some(Observed {
                connected: true,
                active_app: Some("example.app".into()),
                ..Observed::default()
            }),
            choices: vec![],
            cached: false,
            local_elapsed_ms: 42,
        };
        let v = reply(&r);
        assert!(v.get("choices").is_none());
        assert!(v.get("cached").is_none());
        assert_eq!(v["timing_scope"], "application_execution");
        assert_eq!(v["state_scope"], "execution_observation");
        assert!(v.to_string().len() < 320);
    }
}
