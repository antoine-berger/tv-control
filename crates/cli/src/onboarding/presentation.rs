//! Terminal interaction and human/JSON views of unverified discovery evidence.
use serde_json::{json, Value};
use std::io::{self, BufRead, Read, Write};
use tv_adapters::setup;
use tv_domain::onboarding::{Candidate, LocalInterface, ServiceKind, SetupReadiness};

pub(super) fn emit(value: Value, as_json: bool) {
    if as_json {
        println!("{value}");
    } else {
        println!(
            "{}",
            value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Done.")
        );
        if let Some(versions) = value.get("versions") {
            println!(
                "  Invoked CLI: {}; last-started bridge: {} ({}). Running version not verified.",
                versions["invoked_version"].as_str().unwrap_or("unknown"),
                versions["last_started_version"]
                    .as_str()
                    .unwrap_or("unknown"),
                versions["record_status"].as_str().unwrap_or("unknown")
            );
        }
        if let Some(steps) = value.get("next_steps").and_then(Value::as_array) {
            for step in steps {
                if let Some(text) = step.as_str() {
                    println!("  {text}");
                }
            }
        }
        if let Some(commands) = value.get("commands").and_then(Value::as_array) {
            for command in commands {
                if let Some(args) = command.as_array() {
                    let line = args
                        .iter()
                        .filter_map(Value::as_str)
                        .map(setup::shell_quote)
                        .collect::<Result<Vec<_>, _>>();
                    if let Ok(line) = line {
                        println!("  {}", line.join(" "));
                    }
                }
            }
        }
    }
}
pub(super) fn prompt(text: &str) -> Result<String, &'static str> {
    eprint!("{text} ");
    io::stderr().flush().map_err(|_| "terminal_unavailable")?;
    let mut line = String::new();
    let n = io::stdin()
        .lock()
        .take(1025)
        .read_line(&mut line)
        .map_err(|_| "input_unreadable")?;
    if n == 0 {
        return Err("setup_cancelled");
    }
    if n >= 1024 {
        return Err("input_too_long");
    }
    let text = line.trim();
    if text.eq_ignore_ascii_case("q") {
        return Err("setup_cancelled");
    }
    Ok(text.to_string())
}
pub(super) fn choose<T>(
    items: &[T],
    label: impl Fn(&T) -> String,
    title: &str,
) -> Result<usize, &'static str> {
    if items.is_empty() {
        return Err("no_choices_available");
    }
    eprintln!("{title} (q cancels)");
    for (i, item) in items.iter().enumerate() {
        eprintln!("  {}) {}", i + 1, label(item));
    }
    let selected: usize = prompt("Choose a number:")?
        .parse()
        .map_err(|_| "invalid_selection")?;
    if selected == 0 || selected > items.len() {
        return Err("invalid_selection");
    }
    Ok(selected - 1)
}
pub(super) fn interface_json(i: &LocalInterface) -> Value {
    json!({"selector":format!("{}@{}",i.name,i.address),"name":i.name,"address":i.address.to_string(),"netmask":i.netmask.to_string()})
}
fn service_name(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::AdbLegacy => "adb_legacy_unencrypted",
        ServiceKind::AdbTlsConnect => "adb_tls_connect",
        ServiceKind::AdbTlsPairing => "adb_tls_pairing_only",
        ServiceKind::Dial => "dial_advertisement_only",
    }
}
pub(super) fn display_label(c: &Candidate) -> &str {
    if c.services.iter().any(|s| {
        matches!(
            s.kind,
            ServiceKind::AdbLegacy | ServiceKind::AdbTlsConnect | ServiceKind::AdbTlsPairing
        )
    }) {
        if c.label.to_ascii_lowercase().starts_with("adb-") || c.label.is_empty() {
            "Android device (ADB)"
        } else {
            &c.label
        }
    } else {
        "Unidentified media device"
    }
}
pub(super) fn candidate_json(c: &Candidate) -> Value {
    json!({"id":c.id,"label":display_label(c),"diagnostic_label":c.label,"address":c.address.to_string(),"evidence":"advertised_not_verified","authorized":null,"control_verified":false,"services":c.services.iter().map(|s|json!({"kind":service_name(s.kind),"port":s.port,"instance":s.instance})).collect::<Vec<_>>()})
}
pub(super) fn readiness(value: &SetupReadiness) -> &'static str {
    match value {
        SetupReadiness::AuthorizationUnverified => "authorization_unverified",
        SetupReadiness::PairingRequired => "pairing_required",
        SetupReadiness::DebuggingRequired => "debugging_required",
        SetupReadiness::AmbiguousEndpoint => "ambiguous_endpoint",
        SetupReadiness::AddressUnresolved => "address_unresolved",
        SetupReadiness::DiscoveryExpired => "discovery_expired_rediscover",
    }
}
pub(super) fn guidance() -> Vec<&'static str> {
    vec![
 "Detection is an untrusted advertisement, not proof of model, Android version, control support or authorization.",
 "Enable debugging only after reviewing access on the TV. Wireless debugging availability depends on Android TV version/vendor; do not assume Android 12 TV supports TLS pairing.",
 "Fire OS: enable ADB debugging in Developer Options and approve the host on the TV. Vega/non-Android Fire TV models are not supported by this ADB adapter.",
 "A pairing port is temporary and must never be used for control. Pair explicitly using maintained platform-tools, then rediscover the separate TLS connection port.",
 "If nothing appears: check the same LAN, chosen interface, multicast isolation and TV wake state; use the guided manual endpoint fallback only when needed.",
 "Legacy TCP ADB is unencrypted. This setup does not start a daemon, connect, pair or change TV settings."]
}
