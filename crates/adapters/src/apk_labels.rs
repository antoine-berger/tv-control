//! Bounded label-only Android resource reader. No files are extracted or executed.
//! Format reference: AOSP `libs/androidfw/include/androidfw/ResourceTypes.h`.
//! This deliberately does not emulate overlays, split APKs or Android's complete
//! configuration selection. Missing/unsupported labels remain missing.
use std::borrow::Cow;

pub const MAX_RESOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_APK_BYTES: usize = 64 * 1024 * 1024;
const MAX_CHUNKS: usize = 32_768;
const MAX_LABEL_UNITS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApkLabelError {
    Invalid,
    Unsupported,
    TooLarge,
}
type Result<T> = std::result::Result<T, ApkLabelError>;
fn bytes(b: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    b.get(at..at.checked_add(len).ok_or(ApkLabelError::Invalid)?)
        .ok_or(ApkLabelError::Invalid)
}
fn u16le(b: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(bytes(b, at, 2)?.try_into().unwrap()))
}
fn u32le(b: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(b, at, 4)?.try_into().unwrap()))
}
fn chunk(b: &[u8]) -> Result<(u16, usize, &[u8])> {
    let header = u16le(b, 2)? as usize;
    let size = u32le(b, 4)? as usize;
    if header < 8 || size < header {
        return Err(ApkLabelError::Invalid);
    }
    Ok((u16le(b, 0)?, header, bytes(b, 0, size)?))
}

/// Resolve a numeric label resource from a raw resources.arsc table. Exact locale
/// (language/region), language-only, then default; never choose unrelated locales.
/// Only locale-neutral configurations are otherwise considered. References are
/// followed at most eight times, including cycle detection.
pub fn resource_label(
    table: &[u8],
    resource_id: u32,
    locale: Option<&str>,
) -> Result<Option<String>> {
    if table.len() > MAX_RESOURCE_BYTES {
        return Err(ApkLabelError::TooLarge);
    }
    let (kind, header, root) = chunk(table)?;
    if kind != 2 || header < 12 || root.len() != table.len() {
        return Err(ApkLabelError::Invalid);
    }
    let mut pool = None;
    let mut packages = Vec::new();
    let mut at = header;
    let mut count = 0;
    while at < root.len() {
        count += 1;
        if count > MAX_CHUNKS {
            return Err(ApkLabelError::TooLarge);
        }
        let (kind, _, c) = chunk(&root[at..])?;
        match kind {
            1 => {
                if pool.is_some() {
                    return Err(ApkLabelError::Invalid);
                }
                pool = Some(c);
            }
            0x200 => {
                if packages.len() >= 256 {
                    return Err(ApkLabelError::TooLarge);
                }
                packages.push(c);
            }
            _ => {}
        }
        at += c.len();
    }
    if packages.len() != u32le(root, 8)? as usize {
        return Err(ApkLabelError::Invalid);
    }
    let Some(pool) = pool else {
        return Ok(None);
    };
    let mut seen = Vec::new();
    let mut id = resource_id;
    for _ in 0..8 {
        if seen.contains(&id) {
            return Err(ApkLabelError::Invalid);
        }
        seen.push(id);
        let mut best: Option<(u8, u8, u32)> = None;
        for package in &packages {
            if u32le(package, 8)? != id >> 24 {
                continue;
            }
            let (_, ph, _) = chunk(package)?;
            if ph < 284 {
                return Err(ApkLabelError::Invalid);
            }
            let type_offset = if ph >= 288 { u32le(package, 284)? } else { 0 };
            let mut p = ph;
            while p < package.len() {
                count += 1;
                if count > MAX_CHUNKS {
                    return Err(ApkLabelError::TooLarge);
                }
                let (kind, h, c) = chunk(&package[p..])?;
                if kind == 0x201
                    && c.get(8)
                        .copied()
                        .map(u32::from)
                        .and_then(|v| v.checked_add(type_offset))
                        == Some((id >> 16) & 255)
                {
                    if h < 24 {
                        return Err(ApkLabelError::Invalid);
                    }
                    if let Some(rank) = locale_rank(bytes(c, 20, h - 20)?, locale)? {
                        if let Some((ty, data)) = value(c, h, id as u16)? {
                            if best.is_none_or(|b| rank > b.0) {
                                best = Some((rank, ty, data));
                            }
                        }
                    }
                }
                p += c.len();
            }
        }
        match best {
            Some((_, 3, index)) => return pool_string(pool, index).map(Some),
            Some((_, 1, reference)) if reference != 0 => id = reference,
            _ => return Ok(None),
        }
    }
    Err(ApkLabelError::Unsupported)
}

fn locale_rank(config: &[u8], locale: Option<&str>) -> Result<Option<u8>> {
    let size = u32le(config, 0)? as usize;
    if size < 12 || size > config.len() {
        return Err(ApkLabelError::Invalid);
    }
    // Ignore non-locale variants rather than pretending to know device qualifiers.
    if config[4..8]
        .iter()
        .chain(config[12..size].iter())
        .any(|v| *v != 0)
    {
        return Ok(None);
    }
    let lang = bytes(config, 8, 2)?;
    let region = bytes(config, 10, 2)?;
    if lang == [0, 0] && region == [0, 0] {
        return Ok(Some(1));
    }
    // Packed ISO-639-2/UN M.49 encodings are valid but not supported here.
    if lang[0] & 128 != 0 || region[0] & 128 != 0 {
        return Ok(None);
    }
    let Some(locale) = locale else {
        return Ok(None);
    };
    let mut parts = locale.split(['-', '_']);
    let wanted_lang = parts.next().unwrap_or_default();
    let wanted_region = parts.next().unwrap_or_default().trim_start_matches('r');
    if !lang.eq_ignore_ascii_case(wanted_lang.as_bytes()) {
        return Ok(None);
    }
    if region == [0, 0] {
        return Ok(Some(2));
    }
    Ok(region
        .eq_ignore_ascii_case(wanted_region.as_bytes())
        .then_some(3))
}

fn value(c: &[u8], h: usize, id: u16) -> Result<Option<(u8, u32)>> {
    let flags = *c.get(9).ok_or(ApkLabelError::Invalid)?;
    if flags & !3 != 0 || flags == 3 {
        return Err(ApkLabelError::Unsupported);
    }
    let count = u32le(c, 12)? as usize;
    if count > 65_536 {
        return Err(ApkLabelError::TooLarge);
    }
    let start = u32le(c, 16)? as usize;
    let width = if flags == 2 { 2 } else { 4 };
    if start < h + count * width || start > c.len() {
        return Err(ApkLabelError::Invalid);
    }
    let offset = if flags == 1 {
        let mut found = None;
        for i in 0..count {
            if u16le(c, h + i * 4)? == id {
                found = Some(u16le(c, h + i * 4 + 2)? as u32 * 4);
                break;
            }
        }
        match found {
            Some(o) => o,
            None => return Ok(None),
        }
    } else {
        if id as usize >= count {
            return Ok(None);
        }
        if flags == 2 {
            match u16le(c, h + id as usize * 2)? {
                0xffff => return Ok(None),
                v => v as u32 * 4,
            }
        } else {
            u32le(c, h + id as usize * 4)?
        }
    };
    if offset == u32::MAX {
        return Ok(None);
    }
    let entry = bytes(
        c,
        start
            .checked_add(offset as usize)
            .ok_or(ApkLabelError::Invalid)?,
        8,
    )?;
    let flags = u16le(entry, 2)?;
    if flags & 0x00f0 != 0 || (flags & 8 == 0 && flags & 0xff00 != 0) {
        return Err(ApkLabelError::Unsupported);
    }
    if flags & 1 != 0 {
        return Ok(None);
    }
    if flags & 8 != 0 {
        return Ok(Some(((flags >> 8) as u8, u32le(entry, 4)?)));
    }
    let size = u16le(entry, 0)? as usize;
    if size < 8 {
        return Err(ApkLabelError::Invalid);
    }
    let v = bytes(c, start + offset as usize + size, 8)?;
    if u16le(v, 0)? < 8 {
        return Err(ApkLabelError::Invalid);
    }
    Ok(Some((v[3], u32le(v, 4)?)))
}

fn pool_string(pool: &[u8], index: u32) -> Result<String> {
    let (_, h, _) = chunk(pool)?;
    if h < 28 || index >= u32le(pool, 8)? {
        return Err(ApkLabelError::Invalid);
    }
    let count = u32le(pool, 8)? as usize;
    let styles = u32le(pool, 12)? as usize;
    let start = u32le(pool, 20)? as usize;
    if count > MAX_RESOURCE_BYTES / 4
        || styles > MAX_RESOURCE_BYTES / 4
        || start < h + (count + styles) * 4
    {
        return Err(ApkLabelError::Invalid);
    }
    let offset = u32le(pool, h + index as usize * 4)? as usize;
    let styles_start = u32le(pool, 24)? as usize;
    let string_end = if styles_start == 0 {
        pool.len()
    } else {
        styles_start
    };
    if string_end < start || string_end > pool.len() {
        return Err(ApkLabelError::Invalid);
    }
    let pool = &pool[..string_end];
    let mut at = start.checked_add(offset).ok_or(ApkLabelError::Invalid)?;
    let text = if u32le(pool, 16)? & 0x100 != 0 {
        let units = length8(pool, &mut at)?;
        let len = length8(pool, &mut at)?;
        if units > MAX_LABEL_UNITS || len > MAX_LABEL_UNITS * 4 {
            return Err(ApkLabelError::TooLarge);
        }
        let s = std::str::from_utf8(bytes(pool, at, len)?).map_err(|_| ApkLabelError::Invalid)?;
        if bytes(pool, at + len, 1)? != [0] || s.encode_utf16().count() != units {
            return Err(ApkLabelError::Invalid);
        }
        s.to_owned()
    } else {
        let first = u16le(pool, at)?;
        at += 2;
        let units = if first & 0x8000 != 0 {
            let second = u16le(pool, at)?;
            at += 2;
            ((first as usize & 0x7fff) << 16) | second as usize
        } else {
            first as usize
        };
        if units > MAX_LABEL_UNITS {
            return Err(ApkLabelError::TooLarge);
        }
        let data = bytes(pool, at, units * 2)?;
        if u16le(pool, at + units * 2)? != 0 {
            return Err(ApkLabelError::Invalid);
        }
        String::from_utf16(
            &(0..units)
                .map(|i| u16::from_le_bytes([data[i * 2], data[i * 2 + 1]]))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| ApkLabelError::Invalid)?
    };
    if text.trim().is_empty() || text.chars().any(char::is_control) {
        return Err(ApkLabelError::Invalid);
    }
    Ok(text)
}
fn length8(b: &[u8], at: &mut usize) -> Result<usize> {
    let first = bytes(b, *at, 1)?[0];
    *at += 1;
    if first & 128 == 0 {
        Ok(first as usize)
    } else {
        let second = bytes(b, *at, 1)?[0];
        *at += 1;
        Ok(((first as usize & 127) << 8) | second as usize)
    }
}

/// Read only the exact root `resources.arsc` member. ZIP64, encryption,
/// multidisk archives and duplicate resource members are rejected. Nothing is
/// written to disk, so archive paths cannot cause filesystem traversal.
pub fn apk_resource_label(
    apk: &[u8],
    resource_id: u32,
    locale: Option<&str>,
) -> Result<Option<String>> {
    if apk.len() > MAX_APK_BYTES {
        return Err(ApkLabelError::TooLarge);
    }
    let table = extract_table(apk)?;
    match table {
        Some(t) => resource_label(&t, resource_id, locale),
        None => Ok(None),
    }
}
fn extract_table(apk: &[u8]) -> Result<Option<Cow<'_, [u8]>>> {
    let end = apk.len().checked_sub(22).ok_or(ApkLabelError::Invalid)?;
    let eocd = (end.saturating_sub(65535)..=end)
        .rev()
        .find(|&p| {
            apk.get(p..p + 4) == Some(b"PK\x05\x06")
                && u16le(apk, p + 20).is_ok_and(|n| p + 22 + n as usize == apk.len())
        })
        .ok_or(ApkLabelError::Invalid)?;
    if u16le(apk, eocd + 4)? != 0
        || u16le(apk, eocd + 6)? != 0
        || u16le(apk, eocd + 8)? != u16le(apk, eocd + 10)?
    {
        return Err(ApkLabelError::Unsupported);
    }
    let count = u16le(apk, eocd + 10)? as usize;
    if count == 65535 || count > MAX_CHUNKS {
        return Err(ApkLabelError::TooLarge);
    }
    let central_size = u32le(apk, eocd + 12)? as usize;
    let central_start = u32le(apk, eocd + 16)? as usize;
    if central_start.checked_add(central_size) != Some(eocd) {
        return Err(ApkLabelError::Invalid);
    }
    let central = bytes(apk, central_start, central_size)?;
    let mut at = 0;
    let mut found = None;
    for _ in 0..count {
        if bytes(central, at, 4)? != b"PK\x01\x02" {
            return Err(ApkLabelError::Invalid);
        }
        let name_len = u16le(central, at + 28)? as usize;
        let extra_len = u16le(central, at + 30)? as usize;
        let comment_len = u16le(central, at + 32)? as usize;
        let name = bytes(central, at + 46, name_len)?;
        if name == b"resources.arsc" {
            if found.is_some() {
                return Err(ApkLabelError::Invalid);
            }
            let flags = u16le(central, at + 8)?;
            let method = u16le(central, at + 10)?;
            if flags & (1 | 64) != 0 || ![0, 8].contains(&method) || u16le(central, at + 34)? != 0 {
                return Err(ApkLabelError::Unsupported);
            }
            let compressed = u32le(central, at + 20)? as usize;
            let size = u32le(central, at + 24)? as usize;
            if compressed > MAX_RESOURCE_BYTES || size > MAX_RESOURCE_BYTES {
                return Err(ApkLabelError::TooLarge);
            }
            let local = u32le(central, at + 42)? as usize;
            if bytes(apk, local, 4)? != b"PK\x03\x04"
                || u16le(apk, local + 6)? != flags
                || u16le(apk, local + 8)? != method
            {
                return Err(ApkLabelError::Invalid);
            }
            let ln = u16le(apk, local + 26)? as usize;
            let le = u16le(apk, local + 28)? as usize;
            if bytes(apk, local + 30, ln)? != name {
                return Err(ApkLabelError::Invalid);
            }
            let start = local + 30 + ln + le;
            if start
                .checked_add(compressed)
                .is_none_or(|v| v > central_start)
            {
                return Err(ApkLabelError::Invalid);
            }
            let input = bytes(apk, start, compressed)?;
            let output = if method == 0 {
                Cow::Borrowed(input)
            } else {
                Cow::Owned(
                    miniz_oxide::inflate::decompress_to_vec_with_limit(input, size)
                        .map_err(|_| ApkLabelError::Invalid)?,
                )
            };
            if output.len() != size || crc32(&output) != u32le(central, at + 16)? {
                return Err(ApkLabelError::Invalid);
            }
            found = Some(output);
        }
        at = at
            .checked_add(46 + name_len + extra_len + comment_len)
            .ok_or(ApkLabelError::Invalid)?;
        bytes(central, 0, at)?;
    }
    if at != central.len() {
        return Err(ApkLabelError::Invalid);
    }
    Ok(found)
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320u32 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    fn w16(b: &mut [u8], p: usize, v: u16) {
        b[p..p + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn w32(b: &mut [u8], p: usize, v: u32) {
        b[p..p + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn header(kind: u16, h: usize, size: usize) -> Vec<u8> {
        let mut b = vec![0; size];
        w16(&mut b, 0, kind);
        w16(&mut b, 2, h as u16);
        w32(&mut b, 4, size as u32);
        b
    }
    fn pool(strings: &[&str], utf8: bool) -> Vec<u8> {
        let start = 28 + strings.len() * 4;
        let mut b = header(1, 28, start);
        w32(&mut b, 8, strings.len() as u32);
        w32(&mut b, 16, if utf8 { 256 } else { 0 });
        w32(&mut b, 20, start as u32);
        for (i, s) in strings.iter().enumerate() {
            let offset = (b.len() - start) as u32;
            w32(&mut b, 28 + i * 4, offset);
            if utf8 {
                b.push(s.encode_utf16().count() as u8);
                b.push(s.len() as u8);
                b.extend(s.as_bytes());
                b.push(0);
            } else {
                b.extend((s.encode_utf16().count() as u16).to_le_bytes());
                for u in s.encode_utf16() {
                    b.extend(u.to_le_bytes());
                }
                b.extend([0, 0]);
            }
        }
        while b.len() & 3 != 0 {
            b.push(0);
        }
        let len = b.len() as u32;
        w32(&mut b, 4, len);
        b
    }
    fn ty(locale: &[u8; 4], value_type: u8, data: u32, mode: u8, compact: bool) -> Vec<u8> {
        let mut b = header(0x201, 48, if compact { 60 } else { 68 });
        b[8] = 1;
        b[9] = mode;
        w32(&mut b, 12, 1);
        w32(&mut b, 16, 52);
        w32(&mut b, 20, 28);
        b[28..32].copy_from_slice(locale);
        if compact {
            w16(&mut b, 54, 8 | ((value_type as u16) << 8));
            w32(&mut b, 56, data);
        } else {
            w16(&mut b, 52, 8);
            w16(&mut b, 60, 8);
            b[63] = value_type;
            w32(&mut b, 64, data);
        }
        b
    }
    fn table(strings: &[&str], types: &[Vec<u8>], utf8: bool) -> Vec<u8> {
        let mut package = header(0x200, 288, 288);
        w32(&mut package, 8, 0x7f);
        for t in types {
            package.extend(t);
        }
        let n = package.len() as u32;
        w32(&mut package, 4, n);
        let mut b = header(2, 12, 12);
        w32(&mut b, 8, 1);
        b.extend(pool(strings, utf8));
        b.extend(package);
        let n = b.len() as u32;
        w32(&mut b, 4, n);
        b
    }
    fn zip(data: &[u8], compressed: bool, name: &[u8]) -> Vec<u8> {
        let payload = if compressed {
            miniz_oxide::deflate::compress_to_vec(data, 6)
        } else {
            data.to_vec()
        };
        let mut b = vec![0; 30];
        b[..4].copy_from_slice(b"PK\x03\x04");
        w16(&mut b, 4, 20);
        w16(&mut b, 8, if compressed { 8 } else { 0 });
        w32(&mut b, 14, crc32(data));
        w32(&mut b, 18, payload.len() as u32);
        w32(&mut b, 22, data.len() as u32);
        w16(&mut b, 26, name.len() as u16);
        b.extend(name);
        b.extend(&payload);
        let offset = b.len();
        let mut c = vec![0; 46];
        c[..4].copy_from_slice(b"PK\x01\x02");
        w16(&mut c, 6, 20);
        w16(&mut c, 10, if compressed { 8 } else { 0 });
        w32(&mut c, 16, crc32(data));
        w32(&mut c, 20, payload.len() as u32);
        w32(&mut c, 24, data.len() as u32);
        w16(&mut c, 28, name.len() as u16);
        c.extend(name);
        let size = c.len();
        b.extend(c);
        let mut e = vec![0; 22];
        e[..4].copy_from_slice(b"PK\x05\x06");
        w16(&mut e, 8, 1);
        w16(&mut e, 10, 1);
        w32(&mut e, 12, size as u32);
        w32(&mut e, 16, offset as u32);
        b.extend(e);
        b
    }
    #[test]
    fn strings_and_index_formats() {
        for utf8 in [true, false] {
            for mode in [0, 1, 2] {
                for compact in [false, true] {
                    let b = table(
                        &["Télévision 📺"],
                        &[ty(&[0; 4], 3, 0, mode, compact)],
                        utf8,
                    );
                    assert_eq!(
                        resource_label(&b, 0x7f010000, None),
                        Ok(Some("Télévision 📺".into()))
                    );
                    assert_eq!(resource_label(&b, 0x7f010001, None), Ok(None));
                }
            }
        }
    }
    #[test]
    fn locale_falls_back_without_guessing() {
        let b = table(
            &["Default", "Français", "Canada"],
            &[
                ty(&[0; 4], 3, 0, 0, false),
                ty(b"fr\0\0", 3, 1, 0, false),
                ty(b"frCA", 3, 2, 0, false),
            ],
            true,
        );
        for (locale, want) in [
            (None, "Default"),
            (Some("es-PA"), "Default"),
            (Some("fr-FR"), "Français"),
            (Some("fr-CA"), "Canada"),
        ] {
            assert_eq!(
                resource_label(&b, 0x7f010000, locale),
                Ok(Some(want.into()))
            );
        }
        let only_fr = table(&["Français"], &[ty(b"fr\0\0", 3, 0, 0, false)], true);
        assert_eq!(resource_label(&only_fr, 0x7f010000, Some("es")), Ok(None));
    }
    #[test]
    fn rejects_reference_cycles_and_complex_values() {
        let b = table(&["unused"], &[ty(&[0; 4], 1, 0x7f010000, 0, false)], true);
        assert_eq!(
            resource_label(&b, 0x7f010000, None),
            Err(ApkLabelError::Invalid)
        );
        let mut t = ty(&[0; 4], 3, 0, 0, false);
        w16(&mut t, 54, 1);
        let b = table(&["unused"], &[t], true);
        assert_eq!(resource_label(&b, 0x7f010000, None), Ok(None));
    }
    #[test]
    fn zip_stored_deflated_and_traversal() {
        let b = table(&["Actual app"], &[ty(&[0; 4], 3, 0, 0, false)], true);
        for compressed in [false, true] {
            let apk = zip(&b, compressed, b"resources.arsc");
            assert_eq!(
                apk_resource_label(&apk, 0x7f010000, None),
                Ok(Some("Actual app".into()))
            );
        }
        let apk = zip(&b, false, b"../resources.arsc");
        assert_eq!(apk_resource_label(&apk, 0x7f010000, None), Ok(None));
    }
    #[test]
    fn corrupt_zip_and_bomb_are_rejected() {
        let b = table(&["Actual app"], &[ty(&[0; 4], 3, 0, 0, false)], true);
        let mut apk = zip(&b, false, b"resources.arsc");
        apk[43] ^= 1;
        assert_eq!(
            apk_resource_label(&apk, 0x7f010000, None),
            Err(ApkLabelError::Invalid)
        );
        let mut apk = zip(&b, true, b"resources.arsc");
        let central = u32le(&apk, apk.len() - 6).unwrap() as usize;
        w32(&mut apk, central + 24, (MAX_RESOURCE_BYTES + 1) as u32);
        assert_eq!(
            apk_resource_label(&apk, 0x7f010000, None),
            Err(ApkLabelError::TooLarge)
        );
        let mut apk = zip(&b, true, b"resources.arsc");
        let central = u32le(&apk, apk.len() - 6).unwrap() as usize;
        w16(&mut apk, central + 8, 1);
        assert_eq!(
            apk_resource_label(&apk, 0x7f010000, None),
            Err(ApkLabelError::Unsupported)
        );
    }
    #[test]
    fn truncated_and_mutated_inputs_never_panic() {
        let b = table(&["Label"], &[ty(&[0; 4], 3, 0, 0, false)], true);
        for n in 0..b.len() {
            let _ = resource_label(&b[..n], 0x7f010000, None);
        }
        for n in 0..b.len() {
            let mut v = b.clone();
            v[n] = 255;
            let _ = resource_label(&v, 0x7f010000, None);
        }
        let apk = zip(&b, false, b"resources.arsc");
        for n in 0..apk.len() {
            let _ = apk_resource_label(&apk[..n], 0x7f010000, None);
        }
    }
    #[test]
    fn follows_reference_and_handles_nonzero_sparse_id() {
        let mut target = ty(&[0; 4], 3, 0, 1, false);
        target[8] = 2;
        w16(&mut target, 48, 19);
        let b = table(
            &["Resolved label"],
            &[ty(&[0; 4], 1, 0x7f020013, 0, false), target],
            true,
        );
        assert_eq!(
            resource_label(&b, 0x7f010000, None),
            Ok(Some("Resolved label".into()))
        );
    }
    #[test]
    fn rejects_duplicate_resource_members_and_zip_metadata_mutation() {
        let b = table(&["Label"], &[ty(&[0; 4], 3, 0, 0, false)], true);
        let apk = zip(&b, false, b"resources.arsc");
        for n in 0..apk.len() {
            let mut mutated = apk.clone();
            mutated[n] = 255;
            let _ = apk_resource_label(&mutated, 0x7f010000, None);
        }
        let central = u32le(&apk, apk.len() - 6).unwrap() as usize;
        let record = apk[central..apk.len() - 22].to_vec();
        let mut dup = apk[..apk.len() - 22].to_vec();
        dup.extend(&record);
        let mut end = apk[apk.len() - 22..].to_vec();
        w16(&mut end, 8, 2);
        w16(&mut end, 10, 2);
        w32(&mut end, 12, (record.len() * 2) as u32);
        dup.extend(end);
        assert_eq!(
            apk_resource_label(&dup, 0x7f010000, None),
            Err(ApkLabelError::Invalid)
        );
    }
    #[test]
    fn deflate_output_cannot_exceed_declared_size() {
        let b = vec![0; 1024 * 1024];
        let mut apk = zip(&b, true, b"resources.arsc");
        let central = u32le(&apk, apk.len() - 6).unwrap() as usize;
        w32(&mut apk, central + 24, 32);
        assert_eq!(
            apk_resource_label(&apk, 0x7f010000, None),
            Err(ApkLabelError::Invalid)
        );
    }
    #[test]
    fn reads_aosp_aapt2_generated_table() {
        let b = include_bytes!("../tests/fixtures/android/aosp-basic.arsc");
        assert_eq!(
            resource_label(b, 0x7f030000, None),
            Ok(Some("test1".into()))
        );
        assert_eq!(
            resource_label(b, 0x7f030001, Some("es-PA")),
            Ok(Some("test2".into()))
        );
        assert_eq!(resource_label(b, 0x7f040000, None), Ok(None));
    }
    #[test]
    fn rejects_inconsistent_table_metadata() {
        let b = table(&["Label"], &[ty(&[0; 4], 3, 0, 0, false)], true);
        let mut wrong_count = b.clone();
        w32(&mut wrong_count, 8, 2);
        assert_eq!(
            resource_label(&wrong_count, 0x7f010000, None),
            Err(ApkLabelError::Invalid)
        );
        let mut duplicate = b.clone();
        duplicate.extend(pool(&["Other"], true));
        let len = duplicate.len() as u32;
        w32(&mut duplicate, 4, len);
        assert_eq!(
            resource_label(&duplicate, 0x7f010000, None),
            Err(ApkLabelError::Invalid)
        );
        let mut unsupported = ty(&[0; 4], 3, 0, 0, false);
        w16(&mut unsupported, 54, 0x10);
        let b = table(&["Label"], &[unsupported], true);
        assert_eq!(
            resource_label(&b, 0x7f010000, None),
            Err(ApkLabelError::Unsupported)
        );
    }
    #[test]
    fn output_and_input_limits() {
        assert_eq!(
            resource_label(&vec![0; MAX_RESOURCE_BYTES + 1], 1, None),
            Err(ApkLabelError::TooLarge)
        );
        let b = table(&["\n"], &[ty(&[0; 4], 3, 0, 0, false)], true);
        assert_eq!(
            resource_label(&b, 0x7f010000, None),
            Err(ApkLabelError::Invalid)
        );
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
    }
}
