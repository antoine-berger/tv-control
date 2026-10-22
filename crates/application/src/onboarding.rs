//! Discovery and setup policies, independent of UDP, files and UI.
use crate::Context;
use tv_domain::{onboarding::*, Failure, PortResult};
pub trait NetworkDiscovery: Send + Sync {
    fn interfaces(&self) -> PortResult<Vec<LocalInterface>>;
    fn discover(
        &self,
        interface: &LocalInterface,
        window_end_monotonic_ms: u64,
        ctx: &Context<'_>,
    ) -> PortResult<Vec<Candidate>>;
}
pub fn discover(
    port: &dyn NetworkDiscovery,
    interface: &str,
    window_end_monotonic_ms: u64,
    ctx: &Context<'_>,
) -> PortResult<Vec<Candidate>> {
    ctx.check()?;
    let all = port.interfaces()?;
    let matches: Vec<_> = all
        .iter()
        .filter(|i| i.name == interface || format!("{}@{}", i.name, i.address) == interface)
        .collect();
    if matches.len() != 1 {
        return Err(Failure::before("interface_missing_or_ambiguous"));
    }
    let chosen = matches[0];
    let raw = port.discover(chosen, window_end_monotonic_ms, ctx)?;
    ctx.check()?;
    let mut result: Vec<Candidate> = Vec::new();
    for mut candidate in raw.into_iter().take(256) {
        if !chosen.contains(candidate.address) {
            continue;
        }
        candidate.services.retain(|s| s.port != Some(0));
        candidate.services.sort();
        candidate.services.dedup();
        if candidate.services.is_empty() {
            continue;
        }
        if let Some(found) = result.iter_mut().find(|c| c.address == candidate.address) {
            found.services.extend(candidate.services);
            found.services.sort();
            found.services.dedup();
        } else if result.len() < 64 {
            candidate.id = format!("screen_{}", candidate.address.to_string().replace('.', "_"));
            candidate.label = candidate
                .label
                .chars()
                // Preserve diagnostic punctuation (notably SSDP SERVER versions).
                // Labels are data, never commands; terminal controls remain excluded.
                .filter(|c| !c.is_control())
                .take(80)
                .collect();
            if candidate.label.is_empty() {
                candidate.label = "Network display".into();
            }
            result.push(candidate);
        }
    }
    result.sort_by_key(|c| c.address);
    Ok(result)
}
pub fn plan_setup(mut candidate: Candidate, now_monotonic_ms: u64) -> SetupPlan {
    let had_services = !candidate.services.is_empty();
    candidate.services.retain(|s| {
        s.valid_until_monotonic_ms
            .is_none_or(|until| until > now_monotonic_ms)
    });
    let tls: Vec<_> = candidate
        .services
        .iter()
        .filter(|s| s.kind == ServiceKind::AdbTlsConnect)
        .filter_map(|s| s.port)
        .collect();
    let legacy: Vec<_> = candidate
        .services
        .iter()
        .filter(|s| s.kind == ServiceKind::AdbLegacy)
        .filter_map(|s| s.port)
        .collect();
    let ports = if tls.is_empty() { legacy } else { tls };
    let (endpoint, readiness) = match ports.as_slice() {
        [port] => (
            Some(format!("{}:{}", candidate.address, port)),
            SetupReadiness::AuthorizationUnverified,
        ),
        [] if had_services && candidate.services.is_empty() => {
            (None, SetupReadiness::DiscoveryExpired)
        }
        [] if candidate.services.iter().any(|s| {
            matches!(s.kind, ServiceKind::AdbLegacy | ServiceKind::AdbTlsConnect)
                && s.port.is_none()
        }) =>
        {
            (None, SetupReadiness::AddressUnresolved)
        }
        [] if candidate
            .services
            .iter()
            .any(|s| s.kind == ServiceKind::AdbTlsPairing) =>
        {
            (None, SetupReadiness::PairingRequired)
        }
        [] => (None, SetupReadiness::DebuggingRequired),
        _ => (None, SetupReadiness::AmbiguousEndpoint),
    };
    SetupPlan {
        candidate,
        endpoint,
        readiness,
    }
}
