//! LAN selection, bounded discovery, manual endpoints, and advertisement freshness.
use super::presentation::{choose, emit, guidance, prompt};
use super::{fail, Options};
use serde_json::json;
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    path::Path,
};
use tv_adapters::{
    discovery::{fixture::FixtureDiscovery, LocalDiscovery},
    runtime::CancelFlag,
    setup,
};
use tv_application::{
    onboarding::{self, NetworkDiscovery},
    Clock, Context,
};
use tv_domain::onboarding::{AdvertisedService, Candidate, ServiceKind, SetupPlan};

const MAX_DISCOVERY_TIMEOUT_MS: u64 = 10_000;
struct DiscoveryTiming {
    window_ms: u64,
    timeout_ms: u64,
}
impl Default for DiscoveryTiming {
    fn default() -> Self {
        Self {
            window_ms: 2_000,
            timeout_ms: MAX_DISCOVERY_TIMEOUT_MS,
        }
    }
}
impl DiscoveryTiming {
    fn from_options(options: &Options) -> tv_domain::PortResult<Self> {
        let defaults = Self::default();
        let window_ms = options
            .get("--duration-ms")
            .map(str::parse::<u64>)
            .transpose()
            .map_err(|_| tv_domain::Failure::before("invalid_discovery_duration"))?
            .unwrap_or(defaults.window_ms);
        let timeout_ms = options
            .get("--timeout-ms")
            .map(str::parse::<u64>)
            .transpose()
            .map_err(|_| tv_domain::Failure::before("invalid_discovery_timeout"))?
            .unwrap_or(defaults.timeout_ms);
        if !(100..=5000).contains(&window_ms) {
            return Err(tv_domain::Failure::before(
                "discovery_duration_out_of_range",
            ));
        }
        if timeout_ms <= window_ms || timeout_ms > MAX_DISCOVERY_TIMEOUT_MS {
            return Err(tv_domain::Failure::before("discovery_timeout_out_of_range"));
        }
        Ok(Self {
            window_ms,
            timeout_ms,
        })
    }
}
pub(super) fn source(options: &Options) -> Result<Box<dyn NetworkDiscovery>, &'static str> {
    match options.get("--fixture") {
        Some(path) => Ok(Box::new(
            FixtureDiscovery::from_json(&setup::read_bounded(Path::new(path))?).map_err(fail)?,
        )),
        None => Ok(Box::new(LocalDiscovery)),
    }
}
pub(super) fn select_interface(
    options: &Options,
    port: &dyn NetworkDiscovery,
) -> Result<String, &'static str> {
    let interfaces = port.interfaces().map_err(fail)?;
    let selected = if let Some(name) = options.get("--interface") {
        name.to_string()
    } else if options.json {
        return Err("interface_required");
    } else if options.guided && interfaces.len() == 1 {
        format!("{}@{}", interfaces[0].name, interfaces[0].address)
    } else {
        let n = choose(
            &interfaces,
            |i| format!("{} ({})", i.name, i.address),
            "Choose the LAN interface shared with the TV; avoid WAN/VPN:",
        )?;
        format!("{}@{}", interfaces[n].name, interfaces[n].address)
    };
    let matches: Vec<_> = interfaces
        .iter()
        .filter(|i| i.name == selected || format!("{}@{}", i.name, i.address) == selected)
        .collect();
    if matches.len() != 1 {
        return Err("interface_missing_or_ambiguous");
    }
    Ok(format!("{}@{}", matches[0].name, matches[0].address))
}
pub(super) fn discover_on(
    options: &Options,
    port: &dyn NetworkDiscovery,
    clock: &dyn Clock,
    selected: &str,
) -> tv_domain::PortResult<Vec<Candidate>> {
    let timing = DiscoveryTiming::from_options(options)?;
    // Start both bounds only after interface selection and all human input.
    let start = clock.monotonic_ms();
    let window_end = start.saturating_add(timing.window_ms);
    let flag = CancelFlag::default();
    let ctx = Context {
        clock,
        cancellation: &flag,
        deadline_monotonic_ms: start.saturating_add(timing.timeout_ms),
    };
    onboarding::discover(port, selected, window_end, &ctx)
}
pub(super) fn discovery_error(error: tv_domain::Failure, interface: &str, as_json: bool) {
    if error.code == "expired" || error.code == "cancelled" {
        let message = if error.code == "expired" {
            "TV search exceeded its time limit. Retry setup or use the manual fallback."
        } else {
            "TV search cancelled."
        };
        if as_json {
            emit(
                json!({"status":if error.code=="expired"{"timeout"}else{"cancelled"},"code":error.code,"interface":interface,"message":message}),
                true,
            );
        } else {
            eprintln!("{message}");
        }
        return;
    }
    if as_json {
        emit(
            json!({"status":"error","code":error.code,"os_error":error.os_error,"interface":interface,"cause":"not_established","next_steps":["Discovery failed. Resume setup to retry explicitly, or use its guided manual endpoint fallback.","No TV authorization or connection was performed."]}),
            true,
        );
    } else {
        eprintln!(
            "Discovery failed: {} (OS error: {}). The cause is not established.",
            error.code,
            error
                .os_error
                .map(|n| n.to_string())
                .unwrap_or_else(|| "unavailable".into())
        );
    }
}
pub(super) fn manual_candidate(
    options: &Options,
    port: &dyn NetworkDiscovery,
    interactive: bool,
    selected_address: Option<Ipv4Addr>,
) -> Result<Candidate, &'static str> {
    let endpoint = if let Some(value) = options.get("--manual-endpoint") {
        value.to_string()
    } else if interactive {
        eprintln!("{}", guidance().join("\n"));
        if let Some(address) = selected_address {
            let port:u16=prompt("Enter this display's connection port from its debugging settings (not a pairing port), or q:")?.parse().map_err(|_|"invalid_manual_endpoint")?;
            format!("{address}:{port}")
        } else {
            prompt("Enter the TV's connection IPv4:port from its debugging settings (not the pairing port), or q:")?
        }
    } else {
        return Err("no_devices_found");
    };
    let endpoint: SocketAddrV4 = endpoint.parse().map_err(|_| "invalid_manual_endpoint")?;
    let address = *endpoint.ip();
    if endpoint.port() == 0
        || !port
            .interfaces()
            .map_err(fail)?
            .iter()
            .filter(|i| {
                options
                    .get("--interface")
                    .is_none_or(|n| i.name == n || format!("{}@{}", i.name, i.address) == n)
            })
            .any(|i| i.contains(address))
    {
        return Err("manual_endpoint_outside_selected_lan");
    }
    let kind = if let Some(kind) = options.get("--manual-kind") {
        kind.to_string()
    } else if interactive {
        prompt("Connection protocol: type legacy (unencrypted) or tls:")?
    } else {
        return Err("manual_protocol_required");
    };
    let kind = match kind.as_str() {
        "legacy" => ServiceKind::AdbLegacy,
        "tls" => ServiceKind::AdbTlsConnect,
        _ => return Err("invalid_manual_protocol"),
    };
    Ok(Candidate {
        id: format!("screen_{}", address.to_string().replace('.', "_")),
        label: "Manually specified display".into(),
        address,
        services: vec![AdvertisedService {
            kind,
            port: Some(endpoint.port()),
            instance: None,
            valid_until_monotonic_ms: None,
        }],
    })
}
pub(super) fn ensure_fresh(plan: &SetupPlan, clock: &dyn Clock) -> Result<(), &'static str> {
    if plan.candidate.services.iter().any(|s| {
        s.valid_until_monotonic_ms
            .is_some_and(|until| until <= clock.monotonic_ms())
    }) {
        return Err("discovery_expired_rediscover");
    }
    Ok(())
}
pub(super) fn refresh_selected(
    options: &Options,
    port: &dyn NetworkDiscovery,
    clock: &dyn Clock,
    interface: &str,
    selected: &Candidate,
) -> Result<Option<SetupPlan>, &'static str> {
    loop {
        if !options.json {
            eprintln!("The selected display advertisement expired. Refreshing its endpoint on {interface}...");
        }
        let result = discover_on(options, port, clock, interface).and_then(|candidates| {
            onboarding::refresh_setup(selected, candidates, clock.monotonic_ms())
        });
        match result {
            Ok(plan) => return Ok(Some(plan)),
            Err(error) => {
                if options.json {
                    emit(
                        json!({"status":"refresh_required","code":error.code,"os_error":error.os_error,"message":"The selected display could not be revalidated. Retry setup with the same selection; no configuration was saved.","configuration_saved":false,"control_verified":false,"executed":false}),
                        true,
                    );
                    return Ok(None);
                }
                eprintln!("Could not refresh the selected display: {} (OS error: {}). No configuration saved; local setup progress is retained when checkpoints are enabled.", error.code, error.os_error.map(|n| n.to_string()).unwrap_or_else(|| "unavailable".into()));
                if prompt("Retry this display on the same LAN? Enter to retry, or q to stop:")?
                    .is_empty()
                {
                    continue;
                }
                return Err("invalid_selection");
            }
        }
    }
}

#[cfg(test)]
mod tests;
