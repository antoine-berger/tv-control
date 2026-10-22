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
        let mut result = Vec::new();
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
                    dns::collect(&mut result, &data, packet.peer);
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
                result.push(candidate);
            }
        }
        Ok(result)
    }
}
