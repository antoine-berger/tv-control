//! Read-only media service: validation and caller-owned deadlines, no transport knowledge.
use crate::{Cancellation, Clock, Context};
use tv_domain::{media::*, Failure, PortResult};

pub trait Reader: Send + Sync {
    fn read(&self, device: &str, read: &Read, ctx: &Context<'_>) -> PortResult<Data>;
}
pub trait Control: Send + Sync {
    fn read(&self, request: Request, cancel: &dyn Cancellation) -> PortResult<Data>;
}
pub struct Service<R, C> {
    pub reader: R,
    pub clock: C,
    pub max_budget_ms: u64,
}
pub fn media_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
impl<R: Reader, C: Clock> Control for Service<R, C> {
    fn read(&self, request: Request, cancel: &dyn Cancellation) -> PortResult<Data> {
        if !tv_domain::identifier(&request.device)
            || !match &request.read {
                Read::Search { query, limit } => {
                    !query.trim().is_empty()
                        && query.len() <= 500
                        && !query.chars().any(char::is_control)
                        && (1..=50).contains(limit)
                }
                Read::Item { item_id } => media_id(item_id),
                Read::Episodes {
                    series_id,
                    season,
                    limit,
                } => {
                    media_id(series_id)
                        && season.is_none_or(|n| n <= 10000)
                        && (1..=50).contains(limit)
                }
                Read::Playback => true,
            }
        {
            return Err(Failure::before("invalid_media_read"));
        }
        let now = self.clock.unix_ms();
        let remaining = request
            .expires_at_unix_ms
            .checked_sub(now)
            .filter(|v| *v > 0)
            .ok_or(Failure::before("expired"))?;
        if remaining > self.max_budget_ms {
            return Err(Failure::before("budget_exceeds_policy"));
        }
        let ctx = Context {
            clock: &self.clock,
            cancellation: cancel,
            deadline_monotonic_ms: self.clock.monotonic_ms().saturating_add(remaining),
        };
        ctx.check()?;
        let data = self.reader.read(&request.device, &request.read, &ctx)?;
        ctx.check()?;
        Ok(data)
    }
}
