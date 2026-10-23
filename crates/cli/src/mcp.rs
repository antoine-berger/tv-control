//! Narrow JSON-RPC MCP stdio adapter. No listener, installer, browser or embedded model.
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tv_adapters::stdio::{FrameReader, Input, Output};
use tv_adapters::{config::Config, runtime::CancelFlag, wire};
use tv_application::{Cancellation, Clock, Control};
use tv_domain::Status;
#[cfg(test)]
#[path = "mcp_proof_tests.rs"]
mod proof_tests;

fn send(out: &Output, value: Value) {
    out.send(value);
}
fn error(out: &Output, id: Value, code: i32, message: &str) {
    send(
        out,
        json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}),
    );
}
fn tool_result(id: Value, value: Value, is_error: bool) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":is_error}})
}
pub fn tools() -> Value {
    let simple = |name: &str| json!({"type":"object","properties":{"type":{"const":name}},"required":["type"],"additionalProperties":false});
    let named = |name: &str, field: &str| {
        let mut properties = json!({"type":{"const":name}});
        properties[field] = json!({"type":"string","minLength":1,"maxLength":64});
        json!({"type":"object","properties":properties,"required":["type",field],"additionalProperties":false})
    };
    json!({"tools":[
        {"name":"tv_capabilities","description":"List configured aliases and qualified adapter capabilities. No device access.","inputSchema":{"type":"object","properties":{},"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}},
        {"name":"tv_control","description":"Execute one configured TV intent with an absolute expiry. Only status verified proves a postcondition. Volume uses a calibrated Android music stream index, not TV percent.",
        "inputSchema":{"type":"object","properties":{"request_id":{"type":"string","pattern":"^[A-Za-z0-9_-]{1,64}$"},"device":{"type":"string","pattern":"^[A-Za-z0-9_-]{1,64}$"},
            "expires_at_unix_ms":{"type":"integer","minimum":1},"action":{"oneOf":[simple("observe"),simple("pause"),simple("resume"),named("open_app","app"),named("resume_media","media"),
                {"type":"object","properties":{"type":{"const":"set_volume"},"index":{"type":"integer","minimum":0,"maximum":1000}},"required":["type","index"],"additionalProperties":false}]}},
            "required":["request_id","device","action","expires_at_unix_ms"],"additionalProperties":false},
        "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":false,"openWorldHint":false}}
    ]})
}
pub fn serve(control: Arc<dyn Control>, config: Arc<Config>) -> Result<(), &'static str> {
    serve_inner(control, config, None, &CancelFlag::default())
}
#[derive(Clone)]
struct BridgeProof {
    state: std::path::PathBuf,
    snapshot: Arc<Vec<u8>>,
    generation: String,
}
fn complete_verification(
    proof: &BridgeProof,
    nonce: &str,
    result: &tv_domain::Receipt,
    cancel: &dyn Cancellation,
    expires: u64,
) -> Result<tv_adapters::connection_proof::VerificationCode, &'static str> {
    let check = || {
        if cancel.is_cancelled() {
            Err("cancelled")
        } else if tv_adapters::runtime::SystemClock::default().unix_ms() >= expires {
            Err("expired")
        } else {
            Ok(())
        }
    };
    check()?;
    let store = tv_adapters::connection_proof::ProofStore::open(&proof.state.join("verification"))?;
    let code = store.complete(
        nonce,
        &proof.snapshot,
        &proof.generation,
        result,
        tv_adapters::runtime::SystemClock::default().unix_ms(),
    )?;
    if let Err(error) = check() {
        if let Ok(challenge) = store.request(
            nonce,
            &proof.snapshot,
            &proof.generation,
            tv_adapters::runtime::SystemClock::default().unix_ms(),
        ) {
            let _ = store.cancel(&challenge);
        }
        return Err(error);
    }
    Ok(code)
}
pub fn serve_bridge(
    control: Arc<dyn Control>,
    config: Arc<Config>,
    state: std::path::PathBuf,
    snapshot: Vec<u8>,
    generation: String,
    cancellation: &dyn Cancellation,
) -> Result<(), &'static str> {
    serve_inner(
        control,
        config,
        Some(BridgeProof {
            state,
            snapshot: Arc::new(snapshot),
            generation,
        }),
        cancellation,
    )
}
fn serve_inner(
    control: Arc<dyn Control>,
    config: Arc<Config>,
    proof: Option<BridgeProof>,
    cancellation: &dyn Cancellation,
) -> Result<(), &'static str> {
    let mut input = FrameReader::stdin().map_err(|_| "mcp_input_failed")?;
    let (out, output_worker) = Output::start(
        config.preferences.max_queued_responses,
        config.preferences.max_output_bytes,
        config.preferences.output_timeout_ms,
        config.preferences.process_poll_ms,
    )
    .map_err(|_| "mcp_output_failed")?;
    let active: Arc<Mutex<HashMap<String, Arc<CancelFlag>>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut handles = Vec::new();
    let mut initialized = false;
    let mut ready = false;
    loop {
        if out.is_stopped() || cancellation.is_cancelled() {
            break;
        }
        handles.retain(|h: &thread::JoinHandle<()>| !h.is_finished());
        let bytes = match input.next(config.preferences.max_frame_bytes) {
            Ok(Input::Frame(bytes)) => bytes,
            Ok(Input::Idle) => {
                thread::sleep(Duration::from_millis(config.preferences.process_poll_ms));
                continue;
            }
            Ok(Input::TooLarge) => {
                error(&out, Value::Null, -32600, "frame_too_large");
                break;
            }
            Ok(Input::Closed) | Err(_) => break,
        };
        let message: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => {
                error(&out, Value::Null, -32700, "parse_error");
                continue;
            }
        };
        let id = message.get("id").cloned();
        if message.get("jsonrpc") != Some(&json!("2.0"))
            || !message.is_object()
            || id
                .as_ref()
                .is_some_and(|v| !v.is_string() && !v.is_number())
        {
            error(&out, Value::Null, -32600, "invalid_request");
            continue;
        }
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        if id.is_none() {
            match method {
                "notifications/initialized" if initialized => ready = true,
                "notifications/cancelled" => {
                    if let Some(target) = message.pointer("/params/requestId") {
                        if let Ok(active) = active.lock() {
                            if let Some(flag) = active.get(&target.to_string()) {
                                flag.cancel();
                            }
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let id = id.unwrap_or(Value::Null);
        match method {
            "initialize" if !initialized => {
                if !message
                    .pointer("/params/protocolVersion")
                    .is_some_and(Value::is_string)
                    || !message
                        .pointer("/params/capabilities")
                        .is_some_and(Value::is_object)
                    || !message
                        .pointer("/params/clientInfo/name")
                        .is_some_and(Value::is_string)
                    || !message
                        .pointer("/params/clientInfo/version")
                        .is_some_and(Value::is_string)
                {
                    error(&out, id, -32602, "invalid_initialize_parameters");
                    continue;
                }
                initialized = true;
                send(
                    &out,
                    json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"tv-control","version":env!("CARGO_PKG_VERSION")},"instructions":"Configured TV actions only. Never repeat outcome_unknown. Expiry starts at the client, and local_elapsed_ms is not user-to-TV latency."}}),
                );
            }
            "ping" => send(&out, json!({"jsonrpc":"2.0","id":id,"result":{}})),
            _ if !ready => error(&out, id, -32600, "initialize_first"),
            "tools/list" => {
                let mut tools = tools();
                if proof.is_some() {
                    tools["tools"].as_array_mut().unwrap().push(json!({"name":"tv_verify_connection","description":"Make one harmless read-only TV observation for a fresh installer challenge and return its cooperative verification code. Does not attest conversation identity.","inputSchema":{"type":"object","properties":{"challenge":{"type":"string","pattern":"^[a-f0-9]{32}$"}},"required":["challenge"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}}));
                }
                send(&out, json!({"jsonrpc":"2.0","id":id,"result":tools}));
            }
            "tools/call" => {
                let name = message
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let arguments = message
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(json!({}));
                if name == "tv_capabilities" {
                    if arguments != json!({}) {
                        error(&out, id, -32602, "invalid_arguments");
                    } else {
                        send(&out, tool_result(id, super::capabilities(&config), false));
                    }
                    continue;
                }
                let mut verification = None;
                let arguments = if name == "tv_verify_connection" {
                    let Some(proof) = proof.clone() else {
                        error(&out, id, -32602, "verification_unavailable");
                        continue;
                    };
                    if !arguments.is_object() || arguments.as_object().is_some_and(|a| a.len() != 1)
                    {
                        error(&out, id, -32602, "invalid_arguments");
                        continue;
                    }
                    let Some(nonce) = arguments.get("challenge").and_then(Value::as_str) else {
                        error(&out, id, -32602, "invalid_arguments");
                        continue;
                    };
                    let now = tv_adapters::runtime::SystemClock::default().unix_ms();
                    let challenge = match tv_adapters::connection_proof::ProofStore::open(
                        &proof.state.join("verification"),
                    )
                    .and_then(|store| store.request(nonce, &proof.snapshot, &proof.generation, now))
                    {
                        Ok(c) => c,
                        Err(code) => {
                            error(&out, id, -32602, code);
                            continue;
                        }
                    };
                    let args = json!({"request_id":challenge.nonce,"device":challenge.device,"action":{"type":"observe"},"expires_at_unix_ms":challenge.expires_at_unix_ms.min(now.saturating_add(config.preferences.max_budget_ms))});
                    verification = Some((proof, challenge.nonce));
                    args
                } else {
                    arguments
                };
                if name != "tv_control" && name != "tv_verify_connection" {
                    error(&out, id, -32602, "unknown_tool");
                    continue;
                }
                let request: wire::WireRequest = match serde_json::from_value(arguments) {
                    Ok(r) => r,
                    Err(_) => {
                        error(&out, id, -32602, "invalid_arguments");
                        continue;
                    }
                };
                let expires = request.expires_at_unix_ms;
                let key = id.to_string();
                let flag = Arc::new(CancelFlag::default());
                {
                    let mut map = match active.lock() {
                        Ok(map) => map,
                        Err(_) => {
                            out.shutdown();
                            break;
                        }
                    };
                    if map.contains_key(&key) {
                        error(&out, id, -32600, "duplicate_rpc_id");
                        continue;
                    }
                    if map.len() >= config.preferences.max_inflight {
                        error(&out, id, -32000, "server_busy");
                        continue;
                    }
                    map.insert(key.clone(), flag.clone());
                }
                let control = control.clone();
                let out = out.clone();
                let active = active.clone();
                handles.push(thread::spawn(move || {
                    let result = control.execute(request.into(), &*flag);
                    let (value,failed)=if let Some((proof,nonce))=verification {
                        let completed=complete_verification(&proof,&nonce,&result,&*flag,expires);
                        match completed {Ok(code)=>(json!({"verification_code":code.code,"installation_id":code.installation_id,"configuration_revision":code.configuration_revision,"conversation_identity":"cooperative_user_confirmation_required"}),false),Err(code)=>(json!({"code":code,"ready":false}),true)}
                    } else {(wire::receipt(&result),!matches!(result.status,Status::Observed|Status::Verified))};
                    let frame = tool_result(id, value, failed);
                    if failed {
                        out.send(frame);
                    } else {
                        out.send_before(frame, expires);
                    }
                    if let Ok(mut map) = active.lock() {
                        map.remove(&key);
                    }
                }));
            }
            _ => error(&out, id, -32601, "method_not_found"),
        }
    }
    out.shutdown();
    if let Ok(map) = active.lock() {
        for flag in map.values() {
            flag.cancel();
        }
    }
    for handle in handles {
        let _ = handle.join();
    }
    let _ = output_worker.join();
    Ok(())
}
