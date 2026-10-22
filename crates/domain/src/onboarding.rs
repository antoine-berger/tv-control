//! Onboarding vocabulary; addresses are values, never network operations.
use std::net::Ipv4Addr;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalInterface {
    pub name: String,
    pub address: Ipv4Addr,
    pub netmask: Ipv4Addr,
}
impl LocalInterface {
    pub fn contains(&self, address: Ipv4Addr) -> bool {
        let mask = u32::from(self.netmask);
        mask != 0
            && u32::from(self.address) & mask == u32::from(address) & mask
            && address != self.address
            && !address.is_unspecified()
            && !address.is_multicast()
            && !address.is_loopback()
            && u32::from(address) & !mask != !mask
            && u32::from(address) & !mask != 0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ServiceKind {
    AdbLegacy,
    AdbTlsConnect,
    AdbTlsPairing,
    Dial,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AdvertisedService {
    pub kind: ServiceKind,
    pub port: Option<u16>,
    pub instance: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub label: String,
    pub address: Ipv4Addr,
    pub services: Vec<AdvertisedService>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupReadiness {
    AuthorizationUnverified,
    PairingRequired,
    DebuggingRequired,
    AmbiguousEndpoint,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupPlan {
    pub candidate: Candidate,
    pub endpoint: Option<String>,
    pub readiness: SetupReadiness,
}
