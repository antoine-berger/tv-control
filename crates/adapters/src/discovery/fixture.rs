//! Offline wire fixtures exercise exactly the same parsers as the UDP adapter.
use super::{dns, parse_ssdp};
use serde::Deserialize;
use std::net::Ipv4Addr;
use tv_application::{onboarding::NetworkDiscovery, Context};
use tv_domain::{onboarding::*, Failure, PortResult};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Interface {
    name: String,
    address: Ipv4Addr,
    netmask: Ipv4Addr,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Packet {
    protocol: String,
    peer: Ipv4Addr,
    hex: Option<String>,
    text: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureDiscovery {
    interfaces: Vec<Interface>,
    packets: Vec<Packet>,
    failure: Option<FixtureFailure>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureFailure {
    stage: FixtureStage,
    os_error: Option<i32>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum FixtureStage {
    Socket,
    MdnsSend,
    SsdpSend,
    Receive,
}
impl FixtureDiscovery {
    pub fn from_json(bytes: &[u8]) -> PortResult<Self> {
        if bytes.len() > 1_048_576 {
            return Err(Failure::before("fixture_too_large"));
        }
        let fixture: Self =
            serde_json::from_slice(bytes).map_err(|_| Failure::before("invalid_fixture"))?;
        if fixture.interfaces.len() > 16 || fixture.packets.len() > 256 {
            return Err(Failure::before("fixture_too_large"));
        }
        Ok(fixture)
    }
}
impl NetworkDiscovery for FixtureDiscovery {
    fn interfaces(&self) -> PortResult<Vec<LocalInterface>> {
        Ok(self
            .interfaces
            .iter()
            .map(|i| LocalInterface {
                name: i.name.clone(),
                address: i.address,
                netmask: i.netmask,
            })
            .collect())
    }
    fn discover(&self, _: &LocalInterface, ctx: &Context<'_>) -> PortResult<Vec<Candidate>> {
        ctx.check()?;
        if let Some(failure) = &self.failure {
            let code = match failure.stage {
                FixtureStage::Socket => "discovery_socket_failed",
                FixtureStage::MdnsSend => "mdns_send_failed",
                FixtureStage::SsdpSend => "ssdp_send_failed",
                FixtureStage::Receive => "discovery_receive_failed",
            };
            return Err(Failure::before(code).with_os_error(failure.os_error));
        }
        let mut cache = dns::SnapshotCache::default();
        for packet in &self.packets {
            ctx.check()?;
            let candidate = match packet.protocol.as_str() {
                "mdns" => {
                    let hex = packet
                        .hex
                        .as_ref()
                        .ok_or(Failure::before("invalid_fixture"))?;
                    if hex.len() > 16384 || hex.len() % 2 != 0 || !hex.is_ascii() {
                        return Err(Failure::before("invalid_fixture"));
                    }
                    let mut data = Vec::new();
                    for pos in (0..hex.len()).step_by(2) {
                        data.push(
                            u8::from_str_radix(&hex[pos..pos + 2], 16)
                                .map_err(|_| Failure::before("invalid_fixture"))?,
                        );
                    }
                    cache.mdns(&data, packet.peer, ctx.clock.monotonic_ms());
                    None
                }
                "ssdp" => parse_ssdp(
                    packet
                        .text
                        .as_ref()
                        .ok_or(Failure::before("invalid_fixture"))?
                        .as_bytes(),
                    packet.peer,
                ),
                _ => return Err(Failure::before("invalid_fixture_protocol")),
            };
            if let Some(candidate) = candidate {
                cache.dial(candidate, ctx.clock.monotonic_ms());
            }
        }
        Ok(cache.snapshot(ctx.clock.monotonic_ms()))
    }
}
