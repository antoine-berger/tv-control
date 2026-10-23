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

/// The selected control service, excluding pairing and display-only endpoints.
pub fn connection_service(plan: &SetupPlan) -> Option<&AdvertisedService> {
    let endpoint = plan.endpoint.as_ref()?;
    let kind = if plan
        .candidate
        .services
        .iter()
        .any(|s| s.kind == ServiceKind::AdbTlsConnect && s.port.is_some())
    {
        ServiceKind::AdbTlsConnect
    } else {
        ServiceKind::AdbLegacy
    };
    plan.candidate
        .services
        .iter()
        .find(|s| s.kind == kind && s.port.is_some_and(|p| endpoint.ends_with(&format!(":{p}"))))
}

/// Revalidate an expired selection using advertisement identity, never its label.
/// An address alone cannot establish continuity with the selected display.
pub fn refresh_setup(
    selected: &Candidate,
    candidates: Vec<Candidate>,
    now_monotonic_ms: u64,
) -> PortResult<SetupPlan> {
    let original = plan_setup(selected.clone(), 0);
    let service = connection_service(&original)
        .filter(|s| s.instance.as_ref().is_some_and(|s| !s.is_empty()))
        .ok_or(Failure::before("refresh_identity_unavailable"))?;
    let candidate = candidates
        .into_iter()
        .find(|c| c.address == selected.address && c.id == selected.id)
        .ok_or(Failure::before("selected_display_not_found"))?;
    let fresh = plan_setup(candidate, now_monotonic_ms);
    let current =
        connection_service(&fresh).ok_or(Failure::before("selected_endpoint_unavailable"))?;
    if current.kind != service.kind || current.instance != service.instance {
        return Err(Failure::before("selected_display_identity_changed"));
    }
    Ok(fresh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn display(port: u16, until: u64) -> Candidate {
        Candidate {
            id: "screen_192_0_2_10".into(),
            label: "Untrusted label".into(),
            address: Ipv4Addr::new(192, 0, 2, 10),
            services: vec![AdvertisedService {
                kind: ServiceKind::AdbTlsConnect,
                port: Some(port),
                instance: Some("selected._adb-tls-connect._tcp.local".into()),
                valid_until_monotonic_ms: Some(until),
            }],
        }
    }

    #[test]
    fn refresh_requires_current_unique_endpoint_and_advertisement_identity() {
        let selected = display(5555, 1000);
        let fresh = display(9001, 32_000);
        let plan = refresh_setup(&selected, vec![fresh.clone()], 31_000).unwrap();
        assert_eq!(plan.endpoint.as_deref(), Some("192.0.2.10:9001"));
        assert_eq!(plan.readiness, SetupReadiness::AuthorizationUnverified);
        for (candidates, code) in [
            (vec![], "selected_display_not_found"),
            (vec![display(9001, 31_000)], "selected_endpoint_unavailable"),
            (
                {
                    let mut c = fresh.clone();
                    c.services[0].instance = Some("another".into());
                    vec![c]
                },
                "selected_display_identity_changed",
            ),
            (
                {
                    let mut c = fresh.clone();
                    c.services[0].kind = ServiceKind::AdbTlsPairing;
                    vec![c]
                },
                "selected_endpoint_unavailable",
            ),
            (
                {
                    let mut c = fresh.clone();
                    c.services[0].kind = ServiceKind::AdbLegacy;
                    vec![c]
                },
                "selected_display_identity_changed",
            ),
            (
                {
                    let mut c = fresh.clone();
                    let mut s = c.services[0].clone();
                    s.port = Some(9002);
                    c.services.push(s);
                    vec![c]
                },
                "selected_endpoint_unavailable",
            ),
            (
                {
                    let mut c = fresh.clone();
                    c.address = Ipv4Addr::new(192, 0, 2, 20);
                    vec![c]
                },
                "selected_display_not_found",
            ),
        ] {
            assert_eq!(
                refresh_setup(&selected, candidates, 31_000)
                    .unwrap_err()
                    .code,
                code
            );
        }
        let mut unidentified = selected.clone();
        unidentified.services[0].instance = None;
        assert_eq!(
            refresh_setup(&unidentified, vec![fresh], 31_000)
                .unwrap_err()
                .code,
            "refresh_identity_unavailable"
        );
    }
}
