//! Bounded DNS wire parsing and a short-lived unicast discovery snapshot.
use std::net::Ipv4Addr;
use tv_domain::onboarding::*;
pub const SERVICES: [&str; 3] = [
    "_adb._tcp.local",
    "_adb-tls-connect._tcp.local",
    "_adb-tls-pairing._tcp.local",
];
pub fn query() -> Vec<u8> {
    let mut packet = vec![0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0];
    for service in SERVICES {
        for label in service.split('.') {
            packet.push(label.len() as u8);
            packet.extend(label.as_bytes());
        }
        packet.extend([0, 0, 12, 0x80, 1]);
    }
    packet
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Name(Vec<String>);
impl Name {
    fn identity(&self) -> String {
        self.0
            .iter()
            .map(|s| s.replace('\\', "\\\\").replace('.', "\\."))
            .collect::<Vec<_>>()
            .join(".")
    }
    fn service(&self) -> Option<ServiceKind> {
        if self.0.len() != 4 || self.0[2] != "_tcp" || self.0[3] != "local" {
            return None;
        }
        match self.0[1].as_str() {
            "_adb" => Some(ServiceKind::AdbLegacy),
            "_adb-tls-connect" => Some(ServiceKind::AdbTlsConnect),
            "_adb-tls-pairing" => Some(ServiceKind::AdbTlsPairing),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Srv {
    owner: Name,
    target: Name,
    priority: u16,
    weight: u16,
    port: u16,
}
#[derive(Clone, Debug)]
pub enum Record {
    Srv(Srv, u32),
    Address(Name, Ipv4Addr, u32),
}
fn u16_at(data: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(pos..pos + 2)?.try_into().ok()?))
}
fn name(data: &[u8], cursor: &mut usize) -> Option<Name> {
    let mut pos = *cursor;
    let mut jumped = false;
    let mut labels = Vec::new();
    let mut visited = Vec::new();
    for _ in 0..64 {
        if visited.contains(&pos) {
            return None;
        }
        visited.push(pos);
        let len = *data.get(pos)?;
        pos += 1;
        if len & 0xc0 == 0xc0 {
            let dest = (((len & 0x3f) as usize) << 8) | *data.get(pos)? as usize;
            if !jumped {
                *cursor = pos + 1;
                jumped = true;
            }
            pos = dest;
            continue;
        }
        if len & 0xc0 != 0 {
            return None;
        }
        if len == 0 {
            if !jumped {
                *cursor = pos;
            }
            return Some(Name(labels));
        }
        let label = std::str::from_utf8(data.get(pos..pos + len as usize)?).ok()?;
        if label.chars().any(|c| c.is_control()) {
            return None;
        }
        labels.push(label.to_ascii_lowercase());
        pos += len as usize;
        if labels.iter().map(|s| s.len() + 1).sum::<usize>() > 255 {
            return None;
        }
    }
    None
}
pub fn records(data: &[u8]) -> Option<Vec<Record>> {
    if data.len() < 12 || data.len() > 8192 || u16_at(data, 2)? & 0x800f != 0x8000 {
        return None;
    }
    let questions = u16_at(data, 4)? as usize;
    let count =
        (u16_at(data, 6)? as usize) + (u16_at(data, 8)? as usize) + (u16_at(data, 10)? as usize);
    if questions > 32 || count > 128 {
        return None;
    }
    let mut pos = 12;
    for _ in 0..questions {
        name(data, &mut pos)?;
        pos = pos.checked_add(4)?;
        data.get(..pos)?;
    }
    let mut result = Vec::new();
    for _ in 0..count {
        let owner = name(data, &mut pos)?;
        let kind = u16_at(data, pos)?;
        let class = u16_at(data, pos + 2)? & 0x7fff;
        let ttl = u32::from_be_bytes(data.get(pos + 4..pos + 8)?.try_into().ok()?);
        let len = u16_at(data, pos + 8)? as usize;
        pos += 10;
        let end = pos.checked_add(len)?;
        data.get(pos..end)?;
        if class == 1 && kind == 33 && len >= 7 && owner.service().is_some() {
            let mut cursor = pos + 6;
            let target = name(data, &mut cursor)?;
            if cursor != end || target.0.last().map(String::as_str) != Some("local") {
                return None;
            }
            let port = u16_at(data, pos + 4)?;
            if port != 0 {
                result.push(Record::Srv(
                    Srv {
                        owner,
                        target,
                        priority: u16_at(data, pos)?,
                        weight: u16_at(data, pos + 2)?,
                        port,
                    },
                    ttl,
                ));
            }
        } else if class == 1 && kind == 1 && len == 4 {
            result.push(Record::Address(
                owner,
                Ipv4Addr::from(<[u8; 4]>::try_from(&data[pos..end]).ok()?),
                ttl,
            ));
        }
        pos = end;
    }
    Some(result)
}
#[derive(Default)]
pub struct SnapshotCache {
    services: Vec<(Ipv4Addr, Srv, u64)>,
    addresses: Vec<(Ipv4Addr, Name, Ipv4Addr, u64)>,
    dial: Vec<(Candidate, u64)>,
}
fn expiry(now: u64, ttl: u32) -> u64 {
    now.saturating_add((ttl as u64).saturating_mul(1000).min(30_000))
}
impl SnapshotCache {
    pub fn mdns(&mut self, data: &[u8], peer: Ipv4Addr, now: u64) {
        self.prune(now);
        let Some(records) = records(data) else {
            return;
        };
        // Replace a positive RRset once per owner, retaining multiple records in the same packet.
        for record in &records {
            match record {
                Record::Srv(s, ttl) if *ttl > 0 => self
                    .services
                    .retain(|(p, old, _)| *p != peer || old.owner != s.owner),
                Record::Address(n, _, ttl) if *ttl > 0 => self
                    .addresses
                    .retain(|(p, old, _, _)| *p != peer || old != n),
                _ => {}
            }
        }
        for record in records {
            match record {
                Record::Srv(s, 0) => self.services.retain(|(p, old, _)| *p != peer || *old != s),
                Record::Srv(s, ttl) if self.services.len() < 512 => {
                    self.services.push((peer, s, expiry(now, ttl)))
                }
                Record::Address(n, ip, 0) => self
                    .addresses
                    .retain(|(p, old, addr, _)| *p != peer || *old != n || *addr != ip),
                Record::Address(n, ip, ttl) if self.addresses.len() < 256 => {
                    self.addresses.push((peer, n, ip, expiry(now, ttl)))
                }
                _ => {}
            }
        }
    }
    pub fn dial(&mut self, candidate: Candidate, now: u64) {
        self.prune(now);
        if let Some(old) = self
            .dial
            .iter_mut()
            .find(|(c, _)| c.address == candidate.address)
        {
            *old = (candidate, now.saturating_add(30_000));
        } else if self.dial.len() < 64 {
            self.dial.push((candidate, now.saturating_add(30_000)));
        }
    }
    fn prune(&mut self, now: u64) {
        self.services.retain(|(_, _, expires)| *expires > now);
        self.addresses.retain(|(_, _, _, expires)| *expires > now);
        self.dial.retain(|(_, expires)| *expires > now);
    }
    pub fn snapshot(&mut self, now: u64) -> Vec<Candidate> {
        self.prune(now);
        let mut result: Vec<Candidate> = Vec::new();
        for (peer, srv, until) in &self.services {
            let addresses: Vec<_> = self
                .addresses
                .iter()
                .filter(|(p, n, _, _)| p == peer && *n == srv.target)
                .collect();
            let resolved =
                !addresses.is_empty() && addresses.iter().all(|(_, _, ip, _)| ip == peer);
            let until = addresses
                .iter()
                .map(|(_, _, _, e)| *e)
                .fold(*until, u64::min);
            let service = AdvertisedService {
                kind: srv.owner.service().expect("parsed service"),
                port: resolved.then_some(srv.port),
                instance: Some(srv.owner.identity()),
                valid_until_monotonic_ms: Some(until),
            };
            if let Some(c) = result.iter_mut().find(|c| c.address == *peer) {
                if !c.services.contains(&service) && c.services.len() < 16 {
                    c.services.push(service);
                }
            } else if result.len() < 64 {
                result.push(Candidate {
                    id: String::new(),
                    label: srv.owner.0[0].clone(),
                    address: *peer,
                    services: vec![service],
                });
            }
        }
        for (candidate, until) in &self.dial {
            let mut candidate = candidate.clone();
            for s in &mut candidate.services {
                s.valid_until_monotonic_ms = Some(*until);
            }
            if let Some(c) = result.iter_mut().find(|c| c.address == candidate.address) {
                for s in candidate.services {
                    if !c.services.contains(&s) && c.services.len() < 16 {
                        c.services.push(s);
                    }
                }
            } else if result.len() < 64 {
                result.push(candidate);
            }
        }
        result
    }
}
