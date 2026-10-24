//! Non-shipping adapter contract. No concrete transport or deployment is provided.
//! All text is private data: intentionally no Debug/Display on payload-bearing types.
use tv_application::Context;
use tv_domain::{Failure, PortResult};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Disabled,
    ExplicitNativeText,
}

/// Proposed evidence fields only, NOT a trusted capability/authorization token.
/// A future real adapter must validate and bind evidence to the live target/session;
/// these caller-supplied booleans do not establish runtime safety.
pub struct Capability {
    pub sdk: u32,
    pub legacy_runner_verified: bool,
    /// Requires evidence of no activity controller/concurrent automation; not implemented.
    pub lifecycle_preflight_safe: bool,
    pub public_hint_checks: bool,
    pub action_set_text: bool,
    pub device_session: String,
}

/// Issued by a future native observation protocol, NOT synthesized from XML dumps.
/// Deliberately single use: not Clone, and consumed even on uncertain dispatch.
pub struct Request {
    pub device_session: String,
    pub observed_host_ms: u64,
    pub observed_device_uptime_ms: u64,
    pub package: String,
    pub resource: String,
    pub role: String,
    pub window: i32,
    pub bounds: [i32; 4],
    pub before: String,
    pub append: String,
}

#[derive(Clone, Copy)]
pub enum Reply {
    VerifiedAccessibleText,
    Refused,
    Uncertain,
}

/// Implementations must keep payload out of argv/logs, enforce context deadline,
/// own a random 0700 remote directory/0600 request, consume once, and clean up.
/// Any timeout/transport ambiguity is uncertain; never retry or fall back.
/// The Java helper's fixed status key is the only permitted result, not JUnit OK.
pub trait Transport {
    fn run_once(
        &self,
        device_session: &str,
        payload: &[u8],
        ctx: &Context<'_>,
    ) -> PortResult<Reply>;
}

pub fn append_once(
    selection: Selection,
    capability: &Capability,
    request: Request,
    transport: &dyn Transport,
    ctx: &Context<'_>,
) -> PortResult<()> {
    ctx.check()?;
    if selection != Selection::ExplicitNativeText
        || capability.sdk < 26
        || !capability.legacy_runner_verified
        || !capability.lifecycle_preflight_safe
        || !capability.public_hint_checks
        || !capability.action_set_text
    {
        return Err(Failure::before("native_text_unavailable"));
    }
    let now = ctx.clock.monotonic_ms();
    if request.device_session.is_empty()
        || capability.device_session != request.device_session
        || now < request.observed_host_ms
        || now - request.observed_host_ms > 2000
    {
        return Err(Failure::before("native_text_stale"));
    }
    let text_bytes = validated_text_size(&request.before, &request.append)?;
    if [&request.package, &request.resource, &request.role]
        .iter()
        .any(|s| s.is_empty() || s.len() > 2048 || s.chars().any(char::is_control))
        || request.bounds[0] >= request.bounds[2]
        || request.bounds[1] >= request.bounds[3]
        || request.window < 0
    {
        return Err(Failure::before("native_text_invalid"));
    }
    // Allocate only after individual and combined byte/UTF-16 bounds pass.
    let mut after = String::with_capacity(text_bytes);
    after.push_str(&request.before);
    after.push_str(&request.append);
    // Device uptime at observation is conservative: do not add elapsed host time.
    // Both sides enforce <=5 seconds; startup delay can only cause refusal.
    let expires = request
        .observed_device_uptime_ms
        .checked_add(ctx.remaining_ms().min(3000))
        .filter(|v| *v <= i64::MAX as u64)
        .ok_or(Failure::before("native_text_invalid"))?;
    let mut payload = Vec::new();
    payload.extend_from_slice(&0x54564e31u32.to_be_bytes());
    payload.extend_from_slice(&expires.to_be_bytes());
    for s in [&request.package, &request.resource, &request.role] {
        field(&mut payload, s);
    }
    payload.extend_from_slice(&request.window.to_be_bytes());
    for n in request.bounds {
        payload.extend_from_slice(&n.to_be_bytes());
    }
    field(&mut payload, &request.before);
    field(&mut payload, &after);
    ctx.check()?;
    let result = transport.run_once(&request.device_session, &payload, ctx);
    // From transport entry onward all errors are conservative, even purported preflight errors.
    ctx.check().map_err(|e| Failure::uncertain(e.code))?;
    match result {
        Ok(Reply::VerifiedAccessibleText) => Ok(()),
        Ok(Reply::Refused) => Err(Failure::before("native_text_refused")),
        Ok(Reply::Uncertain) | Err(_) => Err(Failure::uncertain("native_text_uncertain")),
    }
}
// Validate borrowed text before allocation or potentially unbounded Unicode scans.
fn validated_text_size(before: &str, append: &str) -> PortResult<usize> {
    const MAX_UNITS: usize = 256;
    const MAX_BYTES: usize = MAX_UNITS * 3; // maximum UTF-8 bytes per UTF-16 unit
    let invalid = || Failure::before("native_text_invalid");
    if append.is_empty() || before.len() > MAX_BYTES || append.len() > MAX_BYTES {
        return Err(invalid());
    }
    let before_units = before.encode_utf16().count();
    let append_units = append.encode_utf16().count();
    if before_units > MAX_UNITS
        || append_units > MAX_UNITS
        || before_units
            .checked_add(append_units)
            .filter(|n| *n <= MAX_UNITS)
            .is_none()
        || before.chars().chain(append.chars()).any(char::is_control)
    {
        return Err(invalid());
    }
    before
        .len()
        .checked_add(append.len())
        .filter(|n| *n <= MAX_BYTES)
        .ok_or_else(invalid)
}

fn field(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod protocol_tests;
