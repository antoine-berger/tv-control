//! Small bounded mDNS record parser; no resolver, URLs or network side effects.
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
            packet.extend_from_slice(label.as_bytes());
        }
        packet.extend_from_slice(&[0, 0, 12, 0x80, 1]); // PTR, unicast response requested
    }
    packet
}
fn u16_at(data: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(pos..pos + 2)?.try_into().ok()?))
}
fn name(data: &[u8], cursor: &mut usize) -> Option<String> {
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
            return Some(labels.join("."));
        }
        let label = std::str::from_utf8(data.get(pos..pos + len as usize)?).ok()?;
        if label.chars().any(|c| c.is_control()) {
            return None;
        }
        labels.push(label.to_string());
        pos += len as usize;
        if labels.iter().map(|s| s.len() + 1).sum::<usize>() > 255 {
            return None;
        }
    }
    None
}
pub fn records(data: &[u8]) -> Option<Vec<(AdvertisedService, u32)>> {
    if data.len() < 12 || data.len() > 8192 || u16_at(data, 2)? & 0x8000 == 0 {
        return None;
    }
    let questions = u16_at(data, 4)? as usize;
    let records =
        (u16_at(data, 6)? as usize) + (u16_at(data, 8)? as usize) + (u16_at(data, 10)? as usize);
    if questions > 32 || records > 128 {
        return None;
    }
    let mut pos = 12;
    for _ in 0..questions {
        name(data, &mut pos)?;
        pos = pos.checked_add(4)?;
        data.get(..pos)?;
    }
    let mut services = Vec::new();
    for _ in 0..records {
        let owner = name(data, &mut pos)?.to_ascii_lowercase();
        let kind = u16_at(data, pos)?;
        let class = u16_at(data, pos + 2)? & 0x7fff;
        let ttl = u32::from_be_bytes(data.get(pos + 4..pos + 8)?.try_into().ok()?);
        let len = u16_at(data, pos + 8)? as usize;
        pos += 10;
        let end = pos.checked_add(len)?;
        data.get(pos..end)?;
        if kind == 33 && class == 1 && len >= 7 {
            for (suffix, kind) in [
                (SERVICES[0], ServiceKind::AdbLegacy),
                (SERVICES[1], ServiceKind::AdbTlsConnect),
                (SERVICES[2], ServiceKind::AdbTlsPairing),
            ] {
                if let Some(_instance) = owner.strip_suffix(&format!(".{suffix}")) {
                    let port = u16_at(data, pos + 4)?;
                    let mut target_pos = pos + 6;
                    let target = name(data, &mut target_pos)?;
                    if target_pos > end
                        || !target.to_ascii_lowercase().ends_with(".local")
                        || port == 0
                    {
                        continue;
                    }
                    services.push((
                        AdvertisedService {
                            kind,
                            port: Some(port),
                            instance: Some(owner.clone()),
                        },
                        ttl,
                    ));
                }
            }
        }
        pos = end;
    }
    Some(services)
}
/// Apply one response including goodbye records and changed dynamic ports.
/// No endpoint is resolved through DNS: a service remains bound to its IPv4 advertiser.
pub fn collect(result: &mut Vec<Candidate>, data: &[u8], peer: Ipv4Addr) {
    let Some(records) = records(data) else {
        return;
    };
    for (service, _) in &records {
        if let Some(candidate) = result.iter_mut().find(|c| c.address == peer) {
            candidate
                .services
                .retain(|s| s.instance != service.instance);
        }
    }
    let mut active: Vec<_> = records
        .into_iter()
        .filter(|(_, ttl)| *ttl != 0)
        .map(|(s, _)| s)
        .take(16)
        .collect();
    if !active.is_empty() {
        if let Some(candidate) = result.iter_mut().find(|c| c.address == peer) {
            candidate.services.append(&mut active);
            candidate.services.truncate(16);
        } else if result.len() < 64 {
            let label = active[0]
                .instance
                .as_ref()
                .and_then(|s| s.split("._adb").next())
                .unwrap_or("Android device")
                .to_string();
            result.push(Candidate {
                id: String::new(),
                label,
                address: peer,
                services: active,
            });
        }
    }
    result.retain(|c| !c.services.is_empty());
}
