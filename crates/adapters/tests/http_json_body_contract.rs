//! JSON commands remain bounded data in stdin, never curl arguments or config directives.
use std::{
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tv_adapters::{
    config::Config,
    http::{CurlHttp, Http, HttpRequest},
    process::{Invocation, Output, Process},
    runtime::{CancelFlag, SystemClock},
};
use tv_application::{Clock, Context};
use tv_domain::*;
#[derive(Default)]
struct Capture(Mutex<Vec<Invocation>>);
impl Process for Capture {
    fn run(&self, i: Invocation, _: &Context<'_>) -> PortResult<Output> {
        self.0.lock().unwrap().push(i);
        Ok(Output {
            stdout: b"\n204".to_vec(),
            success: true,
        })
    }
}
fn request(body: Option<String>) -> HttpRequest {
    HttpRequest {
        server: "library".into(),
        method: "POST",
        path: "/Sessions/session/Command".into(),
        mutating: true,
        json_body: body,
    }
}
fn with_http(test: impl FnOnce(&CurlHttp, &Capture, &Context<'_>)) {
    let mut cfg = Config::from_json(include_bytes!("../../../examples/config.json")).unwrap();
    let variable = format!(
        "TV_HTTP_BODY_FIXTURE_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    cfg.servers.get_mut("library").unwrap().token_env = variable.clone();
    std::env::set_var(&variable, "fixture-token");
    let process = Arc::new(Capture::default());
    let http = CurlHttp {
        config: Arc::new(cfg),
        process: process.clone(),
    };
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    test(
        &http,
        &process,
        &Context {
            clock: &clock,
            cancellation: &cancel,
            deadline_monotonic_ms: clock.monotonic_ms() + 1000,
        },
    );
    std::env::remove_var(variable);
}
#[test]
fn json_quotes_backslashes_and_escaped_newlines_do_not_create_config_lines() {
    with_http(|http, capture, ctx| {
        let body=serde_json::json!({"Name":"SetAudioStreamIndex","Arguments":{"Index":"1","Fixture":"quote\"\\\nheader = bad"}}).to_string();
        assert_eq!(
            http.send(request(Some(body.clone())), ctx).unwrap().status,
            204
        );
        let calls = capture.0.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let i = &calls[0];
        assert!(i.mutating);
        assert_eq!(
            i.args,
            vec!["--disable", "--config", "-"]
                .into_iter()
                .map(Into::into)
                .collect::<Vec<std::ffi::OsString>>()
        );
        let input = String::from_utf8(i.stdin.clone()).unwrap();
        assert!(input.contains(&format!(
            "data = \"{}\"\n",
            body.replace('\\', "\\\\").replace('"', "\\\"")
        )));
        assert_eq!(
            input.lines().filter(|l| l.starts_with("data = ")).count(),
            1
        );
        assert!(input.contains("Content-Type: application/json"));
    });
}
#[test]
fn body_size_json_and_method_validation_reject_before_process() {
    with_http(|http, capture, ctx| {
        for body in [
            "not JSON".to_string(),
            "{}\nheader = bad".into(),
            serde_json::json!({"x":"a".repeat(4096)}).to_string(),
        ] {
            assert_eq!(
                http.send(request(Some(body)), ctx).err().unwrap().code,
                "invalid_http_body"
            );
        }
        let mut get = request(Some("{}".into()));
        get.method = "GET";
        assert_eq!(http.send(get, ctx).err().unwrap().code, "invalid_http_body");
        assert!(capture.0.lock().unwrap().is_empty());
    });
}
