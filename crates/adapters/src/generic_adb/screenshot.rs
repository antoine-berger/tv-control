//! Bounded PNG envelope validation for explicit screenshots.
use tv_domain::{Failure, PortResult};
pub(super) fn validate_png(bytes: &[u8]) -> PortResult<(u32, u32)> {
    let invalid = || Failure::before("screenshot_invalid");
    if bytes.len() > 8 * 1024 * 1024 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(invalid());
    }
    let mut cursor = 8usize;
    let mut dimensions = None;
    let mut data = false;
    let mut palette = false;
    let mut ended_data = false;
    while cursor < bytes.len() {
        let header = bytes
            .get(cursor..cursor.checked_add(8).ok_or_else(invalid)?)
            .ok_or_else(invalid)?;
        let length = u32::from_be_bytes(header[..4].try_into().map_err(|_| invalid())?) as usize;
        let kind = &header[4..8];
        let end = cursor
            .checked_add(12)
            .and_then(|v| v.checked_add(length))
            .ok_or_else(invalid)?;
        let chunk = bytes
            .get(cursor + 8..end.checked_sub(4).ok_or_else(invalid)?)
            .ok_or_else(invalid)?;
        let expected = u32::from_be_bytes(
            bytes
                .get(end - 4..end)
                .ok_or_else(invalid)?
                .try_into()
                .map_err(|_| invalid())?,
        );
        let actual = png_crc(&bytes[cursor + 4..end - 4]);
        if expected != actual || !kind.iter().all(u8::is_ascii_alphabetic) {
            return Err(invalid());
        }
        match kind {
            b"IHDR" => {
                if dimensions.is_some() || cursor != 8 || length != 13 {
                    return Err(invalid());
                }
                let width = u32::from_be_bytes(chunk[..4].try_into().map_err(|_| invalid())?);
                let height = u32::from_be_bytes(chunk[4..8].try_into().map_err(|_| invalid())?);
                let depth = chunk[8];
                let color = chunk[9];
                let valid_depth = match color {
                    0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
                    2 | 4 | 6 => matches!(depth, 8 | 16),
                    3 => matches!(depth, 1 | 2 | 4 | 8),
                    _ => false,
                };
                if width == 0
                    || height == 0
                    || width > 8192
                    || height > 8192
                    || u64::from(width) * u64::from(height) > 16_777_216
                    || !valid_depth
                    || chunk[10] != 0
                    || chunk[11] != 0
                    || chunk[12] > 1
                {
                    return Err(invalid());
                }
                dimensions = Some((width, height, color));
            }
            b"PLTE" => {
                if dimensions.is_none()
                    || data
                    || palette
                    || length == 0
                    || length > 768
                    || length.rem_euclid(3) != 0
                {
                    return Err(invalid());
                }
                palette = true;
            }
            b"IDAT" => {
                let (_, _, color) = dimensions.ok_or_else(invalid)?;
                if ended_data || (color == 3 && !palette) {
                    return Err(invalid());
                }
                data = true;
            }
            b"IEND" => {
                if !data || length != 0 || end != bytes.len() {
                    return Err(invalid());
                }
                let (width, height, _) = dimensions.ok_or_else(invalid)?;
                return Ok((width, height));
            }
            _ => {
                if dimensions.is_none()
                    || kind[0].is_ascii_uppercase()
                    || matches!(kind, b"acTL" | b"fcTL" | b"fdAT")
                {
                    return Err(invalid());
                }
                if data {
                    ended_data = true;
                }
            }
        }
        cursor = end;
    }
    Err(invalid())
}
fn png_crc(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320u32 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
