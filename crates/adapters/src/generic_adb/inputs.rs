//! AOSP TvInputManagerService dump adapter. Text is evidence, never instructions.
use super::{Context, Failure, GenericAdb, PortResult};
use std::collections::{BTreeMap, BTreeSet};
use tv_domain::inputs::{InputConnection, InputListing, TvInput};

const MAX_DUMP: usize = 256 * 1024;
const MAX_INPUTS: usize = 32;
// Fixed text only: merge bounded diagnostics without losing the dumpsys status.
const DUMP_COMMAND: &str = "dumpsys tv_input 2>&1; printf '\nTV_INPUT_DUMP_EXIT=%s\n' \"$?\"";
const EXIT_MARKER: &str = "\nTV_INPUT_DUMP_EXIT=";
fn invalid() -> Failure {
    Failure::unsupported("tv_inputs_format_unsupported")
}
fn incomplete() -> Failure {
    Failure::before("tv_inputs_dump_truncated")
}
impl GenericAdb {
    pub(super) fn read_inputs(&self, alias: &str, ctx: &Context<'_>) -> PortResult<InputListing> {
        // Always fresh; no input selection, helper, catalogue cache or UI reference mutation.
        let bytes = self
            .shell_bytes(
                alias,
                &["sh", "-c", DUMP_COMMAND],
                &[],
                false,
                MAX_DUMP + 64,
                ctx,
            )
            .map_err(|e| match e.code {
                "adb_output_limit" | "process_output_limit" => incomplete(),
                _ => e,
            })?;
        let raw = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
        let listing = parse_framed(raw)?;
        ctx.check()?;
        Ok(listing)
    }
}

fn parse_framed(raw: &str) -> PortResult<InputListing> {
    let (body, status) = raw.rsplit_once(EXIT_MARKER).ok_or_else(incomplete)?;
    if body.contains("TV_INPUT_DUMP_EXIT=") {
        return Err(invalid());
    }
    let status = status.strip_suffix('\n').ok_or_else(incomplete)?;
    let status = number(status)?;
    if status > 255 {
        return Err(invalid());
    }
    if body.len() > MAX_DUMP {
        return Err(incomplete());
    }
    if status != 0 {
        return match parse(body) {
            Err(e)
                if matches!(
                    e.code,
                    "tv_inputs_service_missing" | "tv_inputs_permission_denied"
                ) =>
            {
                Err(e)
            }
            _ => Err(Failure::before("adb_read_failed")),
        };
    }
    parse(body)
}

fn number(raw: &str) -> PortResult<u32> {
    if raw.is_empty() || !raw.bytes().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    raw.parse().map_err(|_| invalid())
}
fn token(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 512
        && raw
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.$/-:".contains(&c))
}
/// Locate a section with an explicit following sibling. A missing terminator is
/// incomplete evidence, not an empty inventory. Repeated headers are ambiguous.
fn section<'a>(lines: &'a [&str], start: &str, end: &str) -> PortResult<&'a [&'a str]> {
    let starts: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| **l == start)
        .collect();
    let [(index, _)] = starts.as_slice() else {
        return Err(if starts.is_empty() {
            incomplete()
        } else {
            invalid()
        });
    };
    let tail = &lines[index + 1..];
    let end = tail.iter().position(|l| *l == end).ok_or_else(incomplete)?;
    Ok(&tail[..end])
}
fn record(line: &str) -> PortResult<&str> {
    let raw = line.strip_prefix("    ").ok_or_else(invalid)?;
    if raw.is_empty() || raw.starts_with(char::is_whitespace) {
        return Err(invalid());
    }
    Ok(raw)
}

fn parse(raw: &str) -> PortResult<InputListing> {
    if raw.len() > MAX_DUMP {
        return Err(incomplete());
    }
    // AOSP dumpsys may return zero after timeout/read/write failures. Never accept
    // an otherwise parseable partial body with one of its anchored diagnostics.
    if raw.lines().any(|line| {
        [
            "*** SERVICE 'tv_input' DUMP TIMEOUT (",
            "Error dumping service info status_t:",
            "Failed to create pipe to dump service info for tv_input:",
            "Error in poll while dumping service tv_input :",
            "Failed to read while dumping service tv_input:",
            "Failed to write while dumping service tv_input:",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix))
    }) {
        return Err(incomplete());
    }
    if raw.trim().is_empty() {
        return Err(Failure::unsupported("tv_inputs_dump_empty"));
    }
    if raw.trim() == "Can't find service: tv_input" {
        return Err(Failure::unsupported("tv_inputs_service_missing"));
    }
    if raw.trim_start().starts_with("Permission Denial:")
        || raw.trim_start().starts_with("Permission denial:")
    {
        return Err(Failure::unsupported("tv_inputs_permission_denied"));
    }
    if !raw.ends_with('\n') {
        return Err(incomplete());
    }
    if raw
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r'))
    {
        return Err(invalid());
    }
    let lines: Vec<_> = raw.lines().collect();
    if lines.iter().any(|l| l.len() > 4096) {
        return Err(invalid());
    }
    let header = lines.first().ok_or_else(invalid)?;
    let current = header
        .strip_prefix("User Ids (Current user: ")
        .and_then(|s| s.strip_suffix("):"))
        .ok_or_else(invalid)?;
    let current_user = number(current)?;
    if lines
        .iter()
        .filter(|l| l.starts_with("User Ids (Current user:"))
        .count()
        != 1
    {
        return Err(invalid());
    }
    let user_header = format!("UserState ({current_user}):");
    let users: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| **l == user_header)
        .collect();
    let [(index, _)] = users.as_slice() else {
        return Err(invalid());
    };
    let user_tail = &lines[index + 1..];
    let user_end = user_tail
        .iter()
        .position(|l| !l.starts_with(' '))
        .ok_or_else(incomplete)?;
    let records = section(
        &user_tail[..user_end],
        "  inputMap: inputId -> TvInputState",
        "  packageSet:",
    )?;
    let mut inputs = BTreeMap::new();
    for line in records {
        let line = record(line)?;
        let (id, rest) = line
            .split_once(": info: TvInputInfo{id=")
            .ok_or_else(invalid)?;
        let (info, state) = rest.split_once("}; state: ").ok_or_else(invalid)?;
        let (info_id, rest) = info.split_once(", pkg=").ok_or_else(invalid)?;
        let (package, service) = rest.split_once(", service=").ok_or_else(invalid)?;
        if id != info_id || !token(id) || !super::valid_component(&format!("{package}/{service}")) {
            return Err(invalid());
        }
        // AOSP INPUT_STATE_* values; these are connection availability, not active source.
        let connection = match state.parse::<i32>().map_err(|_| invalid())? {
            0 => InputConnection::Connected,
            1 => InputConnection::Standby,
            2 => InputConnection::Disconnected,
            _ => InputConnection::Unknown,
        };
        let input = TvInput {
            id: id.into(),
            package: package.into(),
            service: service.into(),
            connection,
            hdmi_port: None,
        };
        if inputs.insert(id, input).is_some() {
            return Err(invalid());
        }
    }
    let hardware_headers: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| **l == "TvInputHardwareManager Info:")
        .collect();
    let [(index, _)] = hardware_headers.as_slice() else {
        return Err(if hardware_headers.is_empty() {
            incomplete()
        } else {
            invalid()
        });
    };
    let hardware = &lines[index + 1..];
    let mut ports = BTreeMap::new();
    for line in section(hardware, "  mHardwareList:", "  mHdmiDeviceList:")? {
        let value = record(line)?
            .strip_prefix("TvInputHardwareInfo {id=")
            .and_then(|s| s.strip_suffix('}'))
            .ok_or_else(invalid)?;
        let (id, rest) = value.split_once(", type=").ok_or_else(invalid)?;
        let id = number(id)?;
        let (kind, rest) = rest.split_once(", audio_type=").ok_or_else(invalid)?;
        let kind = number(kind)?;
        let (_, rest) = rest.split_once(", audio_addr=").ok_or_else(invalid)?;
        // Old releases omit cable_connection_status; audio address is not exposed.
        let rest = rest
            .split_once(", cable_connection_status=")
            .map_or(rest, |(s, _)| s);
        let port = match rest.split_once(", hdmi_port=") {
            Some((_, port)) if kind == 9 => Some(number(port)?),
            None if kind != 9 => None,
            _ => return Err(invalid()),
        };
        if ports.insert(id, port).is_some() {
            return Err(invalid());
        }
    }
    let mut joined_ids = BTreeSet::new();
    let mut joined_inputs = BTreeSet::new();
    for line in section(
        hardware,
        "  mHardwareInputIdMap: deviceId -> inputId",
        "  mHdmiInputIdMap: id -> inputId",
    )? {
        let (id, input_id) = record(line)?.split_once(": ").ok_or_else(invalid)?;
        let id = number(id)?;
        if !token(input_id) || !joined_ids.insert(id) || !joined_inputs.insert(input_id) {
            return Err(invalid());
        }
        if let Some(input) = inputs.get_mut(input_id) {
            input.hdmi_port = ports.get(&id).copied().flatten();
        }
    }
    // A later mandatory sibling distinguishes an interrupted join from complete data.
    section(
        hardware,
        "  mHdmiInputIdMap: id -> inputId",
        "  mInputMap: inputId -> inputInfo",
    )?;
    let truncated = inputs.len() > MAX_INPUTS;
    Ok(InputListing {
        current_user,
        inputs: inputs.into_values().take(MAX_INPUTS).collect(),
        truncated,
    })
}

#[cfg(test)]
mod tests;
