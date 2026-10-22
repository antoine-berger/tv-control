//! Narrow JSON-RPC MCP stdio adapter. No listener, installer, browser or embedded model.
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{self, BufRead, Read, Write},
    sync::{Arc, Mutex},
    thread,
};
use tv_adapters::{config::Config, runtime::CancelFlag, wire};
use tv_application::Control;
use tv_domain::Status;

fn send(out: &Mutex<io::Stdout>, value: Value) {
    if let Ok(mut out) = out.lock() {
        let _ = serde_json::to_writer(&mut *out, &value);
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
}
fn error(out: &Mutex<io::Stdout>, id: Value, code: i32, message: &str) {
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
    let out = Arc::new(Mutex::new(io::stdout()));
    let active: Arc<Mutex<HashMap<String, Arc<CancelFlag>>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut handles = Vec::new();
    let input = io::stdin();
    let mut input = input.lock();
    let mut initialized = false;
    let mut ready = false;
    loop {
        handles.retain(|h: &thread::JoinHandle<()>| !h.is_finished());
        let mut bytes = Vec::new();
        let n = input
            .by_ref()
            .take(config.preferences.max_frame_bytes as u64 + 1)
            .read_until(b'\n', &mut bytes)
            .map_err(|_| "mcp_input_failed")?;
        if n == 0 {
            break;
        }
        if n > config.preferences.max_frame_bytes {
            error(&out, Value::Null, -32600, "frame_too_large");
            break;
        }
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
            "tools/list" => send(&out, json!({"jsonrpc":"2.0","id":id,"result":tools()})),
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
                if name != "tv_control" {
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
                let key = id.to_string();
                let flag = Arc::new(CancelFlag::default());
                {
                    let mut map = active.lock().map_err(|_| "mcp_state_failed")?;
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
                    let failed = !matches!(result.status, Status::Observed | Status::Verified);
                    send(&out, tool_result(id, wire::receipt(&result), failed));
                    if let Ok(mut map) = active.lock() {
                        map.remove(&key);
                    }
                }));
            }
            _ => error(&out, id, -32601, "method_not_found"),
        }
    }
    if let Ok(map) = active.lock() {
        for flag in map.values() {
            flag.cancel();
        }
    }
    for handle in handles {
        let _ = handle.join();
    }
    Ok(())
}
