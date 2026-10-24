//! Generic TV ports and deterministic orchestration, independent of transport.
use crate::{Cancellation, Context};
use tv_domain::{generic::*, Observed, PortResult};

pub trait Device: Send + Sync {
    fn resource(&self, device: &str) -> PortResult<String>;
    fn authorized(&self, device: &str) -> PortResult<()>;
    fn apps(&self, device: &str, ctx: &Context<'_>) -> PortResult<Vec<App>>;
    /// Query is only a hydration priority hint; resolution still uses observed labels.
    fn apps_for(
        &self,
        device: &str,
        _query: Option<&str>,
        ctx: &Context<'_>,
    ) -> PortResult<Vec<App>> {
        self.apps(device, ctx)
    }
    fn app_details(&self, _device: &str, _app: &App, _ctx: &Context<'_>) -> PortResult<AppDetails> {
        Err(tv_domain::Failure::before("unsupported"))
    }
    fn observe(&self, device: &str, ctx: &Context<'_>) -> PortResult<Observed>;
    fn observe_for(
        &self,
        device: &str,
        _intent: &Intent,
        ctx: &Context<'_>,
    ) -> PortResult<Observed> {
        self.observe(device, ctx)
    }
    fn media(&self, _device: &str, _ctx: &Context<'_>) -> PortResult<Vec<MediaSession>> {
        Err(tv_domain::Failure::before("unsupported"))
    }
    fn ui(&self, device: &str, ctx: &Context<'_>) -> PortResult<UiSnapshot>;
    fn remote_capabilities(
        &self,
        _device: &str,
        _ctx: &Context<'_>,
    ) -> PortResult<RemoteCapabilities> {
        Err(tv_domain::Failure::before("unsupported"))
    }
    fn screenshot(&self, _device: &str, _ctx: &Context<'_>) -> PortResult<Screenshot> {
        Err(tv_domain::Failure::before("screenshot_unsupported"))
    }
    /// A resolved app is passed only for OpenApp/OpenUrl/Search. Other intents use None.
    /// TypeText may succeed only after matching accessibility-dump append readback; a
    /// dispatched but unreadable/mismatched insertion must return uncertain failure.
    /// This is an observation, never proof of underlying committed editor text.
    fn apply(
        &self,
        device: &str,
        intent: &Intent,
        app: Option<&App>,
        ctx: &Context<'_>,
    ) -> PortResult<()>;
}
pub enum Reservation {
    Acquired,
    Existing(Operation),
    Conflict,
    Busy,
}
pub trait Journal: Send + Sync {
    /// Must durably record intent before returning Acquired. Cross-process exclusion required.
    fn reserve(
        &self,
        request: &Request,
        resource: &str,
        now_unix_ms: u64,
    ) -> PortResult<Reservation>;
    fn finish(&self, request: &Request, reply: &Reply, now_unix_ms: u64) -> PortResult<()>;
    fn get(&self, id: &str) -> PortResult<Operation>;
    fn cancel(&self, id: &str) -> PortResult<Operation>;
    fn is_cancelled(&self, id: &str) -> bool;
}
pub trait Control: Send + Sync {
    fn read(&self, request: ReadRequest, cancel: &dyn Cancellation) -> PortResult<Data>;
    fn execute(&self, request: Request, cancel: &dyn Cancellation) -> Reply;
    fn operation(&self, id: &str, cancel: bool) -> PortResult<Operation>;
}

use crate::Clock;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tv_domain::{identifier, Effect, Failure, Playback, Status};

struct CachedApps {
    apps: Vec<App>,
    revision: String,
    at: u64,
}
pub struct Service<D, C> {
    device: D,
    journal: Arc<dyn Journal>,
    clock: C,
    max_budget_ms: u64,
    poll_ms: u64,
    cache: Mutex<BTreeMap<String, CachedApps>>,
}
impl<D: Device, C: Clock> Service<D, C> {
    pub fn new(
        device: D,
        journal: Arc<dyn Journal>,
        clock: C,
        max_budget_ms: u64,
        poll_ms: u64,
    ) -> Self {
        Self {
            device,
            journal,
            clock,
            max_budget_ms,
            poll_ms,
            cache: Mutex::new(BTreeMap::new()),
        }
    }
    fn context<'a>(&'a self, expiry: u64, cancel: &'a dyn Cancellation) -> PortResult<Context<'a>> {
        let now = self.clock.unix_ms();
        if expiry <= now {
            return Err(Failure::before("expired"));
        }
        let remaining = expiry - now;
        if remaining > self.max_budget_ms {
            return Err(Failure::before("budget_exceeds_policy"));
        }
        let ctx = Context {
            clock: &self.clock,
            cancellation: cancel,
            deadline_monotonic_ms: self.clock.monotonic_ms().saturating_add(remaining),
        };
        ctx.check()?;
        Ok(ctx)
    }
    fn catalogue(
        &self,
        device: &str,
        refresh: bool,
        query: Option<&str>,
        ctx: &Context<'_>,
    ) -> PortResult<(Vec<App>, String)> {
        ctx.check()?;
        if !refresh {
            let cache = self
                .cache
                .lock()
                .map_err(|_| Failure::before("catalogue_unavailable"))?;
            if let Some(c) = cache.get(device) {
                if self.clock.monotonic_ms().saturating_sub(c.at) < 60_000 {
                    return Ok((c.apps.clone(), c.revision.clone()));
                }
            }
        }
        let mut apps = self.device.apps_for(device, query, ctx)?;
        ctx.check()?;
        if apps.len() > 256 {
            return Err(Failure::before("catalogue_limit"));
        }
        apps.sort_by(|a, b| a.id.cmp(&b.id).then(a.component.cmp(&b.component)));
        apps.dedup();
        let revision = catalogue_revision(&apps);
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| Failure::before("catalogue_unavailable"))?;
        if cache.len() >= 64 && !cache.contains_key(device) {
            return Err(Failure::before("catalogue_limit"));
        }
        cache.insert(
            device.to_string(),
            CachedApps {
                apps: apps.clone(),
                revision: revision.clone(),
                at: self.clock.monotonic_ms(),
            },
        );
        Ok((apps, revision))
    }
    fn run(&self, request: &Request, ctx: &Context<'_>, reply: &mut Reply) -> PortResult<()> {
        ctx.check()?;
        let mut resolved = None;
        if let Intent::OpenApp { app } | Intent::OpenUrl { app, .. } | Intent::Search { app, .. } =
            &request.intent
        {
            let discovery_ctx = Context {
                clock: ctx.clock,
                cancellation: ctx.cancellation,
                deadline_monotonic_ms: self
                    .clock
                    .monotonic_ms()
                    .saturating_add((ctx.remaining_ms() / 2).min(3_000)),
            };
            let was_cached = self
                .cache
                .lock()
                .map_err(|_| Failure::before("catalogue_unavailable"))?
                .get(&request.device)
                .is_some_and(|c| self.clock.monotonic_ms().saturating_sub(c.at) < 60_000);
            let (mut apps, _) =
                self.catalogue(&request.device, false, Some(app), &discovery_ctx)?;
            let mut matches = resolve_apps(&apps, app);
            if matches.is_empty() && was_cached {
                // Refresh one stale miss, never repeat a cold scan in this operation.
                apps = self
                    .catalogue(&request.device, true, Some(app), &discovery_ctx)?
                    .0;
                matches = resolve_apps(&apps, app);
            }
            match matches.len() {
                1 => resolved = matches.into_iter().next(),
                0 => {
                    return Err(Failure::before(if apps.iter().any(|a| a.label.is_none()) {
                        "catalogue_incomplete"
                    } else {
                        "app_not_found"
                    }))
                }
                _ => {
                    reply.choices = matches.into_iter().take(5).collect();
                    return Err(Failure::before("app_ambiguous"));
                }
            }
        }
        ctx.check()?;
        // Native intents have a precise postcondition; navigation and search only
        // promise dispatch plus a fresh observation, never content playback.
        let before = self
            .device
            .observe_for(&request.device, &request.intent, ctx)?;
        ctx.check()?;
        // Keep only samples completed while the operation was live. This is
        // execution evidence, not a promise that the state still holds later.
        reply.observed = Some(diagnostic_state(&before));
        if !before.connected {
            return Err(Failure::before("device_not_ready"));
        }
        let explicit_component =
            matches!(&request.intent, Intent::OpenApp { app } if app.contains('/'));
        if !explicit_component && desired(&request.intent, resolved.as_ref(), &before) {
            reply.status = Status::Verified;
            reply.code = "state_verified".into();
            reply.observed = Some(before);
            return Ok(());
        }
        // A pre-dispatch sample cannot describe an uncertain mutation.
        reply.observed = None;
        self.device
            .apply(&request.device, &request.intent, resolved.as_ref(), ctx)?;
        loop {
            ctx.check().map_err(|e| Failure::uncertain(e.code))?;
            let observed = self
                .device
                .observe_for(&request.device, &request.intent, ctx)
                .map_err(|e| Failure::uncertain(e.code))?;
            ctx.check().map_err(|e| Failure::uncertain(e.code))?;
            reply.observed = Some(diagnostic_state(&observed));
            if request.intent == Intent::Standby
                && (!observed.connected || observed.awake.is_none())
            {
                return Err(Failure::uncertain("standby_unverified"));
            }
            if request.intent == Intent::Wake && observed.awake.is_none() {
                reply.status = Status::Observed;
                reply.code = "wake_dispatched_unverified".into();
                reply.observed = Some(observed);
                return Ok(());
            }
            if matches!(
                request.intent,
                Intent::Navigate { .. }
                    | Intent::LongSelect
                    | Intent::Search { .. }
                    | Intent::OpenUrl { .. }
                    | Intent::Activate { .. }
                    | Intent::Media {
                        command: MediaCommand::Next | MediaCommand::Previous,
                        ..
                    }
                    | Intent::TypeText { .. }
            ) {
                reply.status = Status::Observed;
                reply.code = "command_dispatched_state_observed".into();
                reply.observed = Some(observed);
                return Ok(());
            }
            if desired(&request.intent, resolved.as_ref(), &observed) {
                reply.status = if explicit_component {
                    Status::Observed
                } else {
                    Status::Verified
                };
                reply.code = if explicit_component {
                    "command_dispatched_app_foreground"
                } else {
                    "state_verified"
                }
                .into();
                reply.observed = Some(observed);
                return Ok(());
            }
            ctx.wait(self.poll_ms)
                .map_err(|e| Failure::uncertain(e.code))?;
        }
    }
}
impl<D: Device, C: Clock> Control for Service<D, C> {
    fn read(&self, req: ReadRequest, cancel: &dyn Cancellation) -> PortResult<Data> {
        if !identifier(&req.device) {
            return Err(Failure::before("invalid_device"));
        }
        self.device.resource(&req.device)?;
        let ctx = self.context(req.expires_at_unix_ms, cancel)?;
        let result = match req.read {
            Read::AppDetails { app } => {
                if !valid_text(&app, 512) {
                    return Err(Failure::before("invalid_app"));
                }
                // Refresh identity rather than reusing a potentially removed/replaced app.
                // Details accept only exact discovered IDs/components, never fuzzy labels.
                let (apps, _) = self.catalogue(&req.device, true, Some(&app), &ctx)?;
                let matches: Vec<_> = apps
                    .iter()
                    .filter(|a| a.id == app || a.component == app)
                    .collect();
                let selected = match matches.as_slice() {
                    [selected] => *selected,
                    [] => return Err(Failure::before("app_not_found")),
                    _ => return Err(Failure::before("app_ambiguous")),
                };
                Data::AppDetails(self.device.app_details(&req.device, selected, &ctx)?)
            }
            Read::Media => {
                let mut sessions = self.device.media(&req.device, &ctx)?;
                let truncated = sessions.len() > 8;
                sessions.truncate(8);
                Data::Media {
                    sessions,
                    truncated,
                }
            }
            Read::RemoteCapabilities => {
                Data::RemoteCapabilities(self.device.remote_capabilities(&req.device, &ctx)?)
            }
            Read::Power => Data::State(self.device.observe_for(
                &req.device,
                &Intent::Standby,
                &ctx,
            )?),
            Read::Volume => {
                Data::State(self.device.observe_for(&req.device, &Intent::Mute, &ctx)?)
            }
            Read::State => Data::State(self.device.observe(&req.device, &ctx)?),
            Read::Ui => Data::Ui(self.device.ui(&req.device, &ctx)?),
            Read::Screenshot => Data::Screenshot(self.device.screenshot(&req.device, &ctx)?),
            Read::Apps {
                query,
                since,
                limit,
                refresh,
            } => {
                if !(1..=32).contains(&limit)
                    || query.as_ref().is_some_and(|q| !valid_text(q, 200))
                    || since.as_ref().is_some_and(|s| s.len() > 64)
                {
                    return Err(Failure::before("invalid_catalogue_query"));
                }
                let (apps, revision) =
                    self.catalogue(&req.device, refresh, query.as_deref(), &ctx)?;
                let unchanged = since.as_ref() == Some(&revision);
                let mut filtered: Vec<_> = if unchanged {
                    vec![]
                } else {
                    apps.into_iter()
                        .filter(|a| {
                            query.as_ref().is_none_or(|q| {
                                let q = normalize(q);
                                normalize(&a.id).contains(&q)
                                    || a.label.as_ref().is_some_and(|l| normalize(l).contains(&q))
                            })
                        })
                        .collect()
                };
                let truncated = filtered.len() > limit;
                filtered.truncate(limit);
                Data::Apps {
                    revision,
                    unchanged,
                    apps: filtered,
                    truncated,
                }
            }
        };
        ctx.check()?;
        Ok(result)
    }
    fn execute(&self, req: Request, cancel: &dyn Cancellation) -> Reply {
        let start = self.clock.monotonic_ms();
        let mut reply = Reply {
            request_id: req.id.clone(),
            status: Status::Invalid,
            code: "invalid_request".into(),
            observed: None,
            choices: vec![],
            cached: false,
            local_elapsed_ms: 0,
        };
        if !identifier(&req.id) || !identifier(&req.device) || !valid_intent(&req.intent) {
            return reply;
        }
        let resource = match self
            .device
            .resource(&req.device)
            .and_then(|r| self.device.authorized(&req.device).map(|_| r))
        {
            Ok(r) => r,
            Err(e) => {
                fail(&mut reply, e);
                return reply;
            }
        };
        let initial_ctx = match self.context(req.expires_at_unix_ms, cancel) {
            Ok(c) => c,
            Err(e) => {
                fail(&mut reply, e);
                return reply;
            }
        };
        match self.journal.reserve(&req, &resource, self.clock.unix_ms()) {
            Ok(Reservation::Acquired) => {}
            Ok(Reservation::Existing(Operation::Complete(mut r))) => {
                r.cached = true;
                return r;
            }
            Ok(Reservation::Existing(_) | Reservation::Busy) => {
                reply.status = Status::Busy;
                reply.code = "operation_busy".into();
                return reply;
            }
            Ok(Reservation::Conflict) => {
                reply.code = "request_id_conflict".into();
                return reply;
            }
            Err(e) => {
                fail(&mut reply, e);
                return reply;
            }
        }
        let combined = CombinedCancellation {
            outer: cancel,
            journal: &*self.journal,
            id: &req.id,
        };
        let ctx = Context {
            clock: &self.clock,
            cancellation: &combined,
            deadline_monotonic_ms: initial_ctx.deadline_monotonic_ms,
        };
        if let Err(e) = self.run(&req, &ctx, &mut reply) {
            if e.code == "app_not_launchable" || e.code == "adb_command_failed" {
                if let Ok(mut cache) = self.cache.lock() {
                    cache.remove(&req.device);
                }
            }
            fail(&mut reply, e);
        }
        reply.local_elapsed_ms = self.clock.monotonic_ms().saturating_sub(start);
        if self
            .journal
            .finish(&req, &reply, self.clock.unix_ms())
            .is_err()
        {
            reply.observed = reply.observed.as_ref().map(diagnostic_state);
            reply.status = Status::OutcomeUnknown;
            reply.code = "operation_persistence_failed".into();
        }
        reply
    }
    fn operation(&self, id: &str, cancel: bool) -> PortResult<Operation> {
        if !identifier(id) {
            return Err(Failure::before("invalid_request_id"));
        }
        if cancel {
            self.journal.cancel(id)
        } else {
            self.journal.get(id)
        }
    }
}
struct CombinedCancellation<'a> {
    outer: &'a dyn Cancellation,
    journal: &'a dyn Journal,
    id: &'a str,
}
impl Cancellation for CombinedCancellation<'_> {
    fn is_cancelled(&self) -> bool {
        self.outer.is_cancelled() || self.journal.is_cancelled(self.id)
    }
}
fn valid_text(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
fn valid_intent(intent: &Intent) -> bool {
    match intent {
        Intent::Media { session, .. } => valid_media_session(session),
        Intent::Legacy { .. } => false,
        Intent::OpenApp { app } => valid_text(app, 512),
        Intent::OpenUrl { app, url } => valid_text(app, 512) && valid_url(url),
        Intent::Search { app, query } => valid_text(app, 512) && valid_text(query, 500),
        Intent::Navigate { keys } => !keys.is_empty() && keys.len() <= 16,
        Intent::Activate { reference } => valid_text(reference, 200),
        Intent::TypeText { reference, text } => valid_text(reference, 200) && valid_text(text, 500),
        Intent::SetVolume { index } => *index <= 1000,
        Intent::Wake
        | Intent::Standby
        | Intent::Mute
        | Intent::Pause
        | Intent::Resume
        | Intent::LongSelect => true,
    }
}
fn valid_url(url: &str) -> bool {
    if !valid_text(url, 2048) || url.chars().any(char::is_whitespace) {
        return false;
    }
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    !authority.is_empty() && !authority.contains('@') && !authority.contains('\\')
}
fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn resolve_apps(apps: &[App], query: &str) -> Vec<App> {
    let exact: Vec<_> = apps
        .iter()
        .filter(|a| a.id == query || a.component == query)
        .cloned()
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let q = normalize(query);
    apps.iter()
        .filter(|a| a.label.as_ref().is_some_and(|l| normalize(l) == q))
        .cloned()
        .collect()
}
fn desired(intent: &Intent, app: Option<&App>, state: &Observed) -> bool {
    if !state.connected {
        return false;
    }
    match intent {
        Intent::OpenApp { .. } => app.is_some_and(|a| state.active_app.as_ref() == Some(&a.id)),
        Intent::Wake => state.awake == Some(true),
        Intent::Standby => state.connected && state.awake == Some(false),
        Intent::Mute => state.volume_index == Some(0),
        Intent::SetVolume { index } => state.volume_index == Some(*index),
        Intent::Pause => state.playback == Playback::Paused,
        Intent::Media {
            command: MediaCommand::Pause,
            ..
        } => {
            state.playback == Playback::Paused
                && state.playback_evidence == tv_domain::PlaybackEvidence::NativeState
        }
        Intent::Resume
        | Intent::Media {
            command: MediaCommand::Resume,
            ..
        } => {
            state.playback == Playback::Playing
                && state.playback_evidence == tv_domain::PlaybackEvidence::NativeState
        }
        _ => false,
    }
}
/// Bounded operational evidence only: never persist media identity/position
/// merely because verification failed. Adapter text is not trusted metadata.
fn diagnostic_state(state: &Observed) -> Observed {
    Observed {
        connected: state.connected,
        active_app: state
            .active_app
            .as_ref()
            .filter(|app| {
                !app.is_empty()
                    && app.len() <= 255
                    && app
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.".contains(&b))
            })
            .cloned(),
        playback: state.playback,
        playback_evidence: state.playback_evidence,
        awake: state.awake,
        volume_index: state.volume_index,
        volume_max_index: state.volume_max_index,
        ..Observed::default()
    }
}
fn fail(reply: &mut Reply, e: Failure) {
    reply.code = e.code.into();
    reply.status = if e.effect == Effect::MayHaveChanged {
        Status::OutcomeUnknown
    } else {
        match e.code {
            "expired" => Status::Expired,
            "cancelled" => Status::Cancelled,
            "unsupported"
            | "standby_unsupported"
            | "search_unsupported"
            | "url_unsupported"
            | "input_text_unsupported"
            | "text_verification_unsupported"
            | "media_command_unsupported" => Status::Unsupported,
            _ => Status::Failed,
        }
    };
}
fn catalogue_revision(apps: &[App]) -> String {
    // Revision is only a change detector, not an authorization or UI token.
    let mut hash = 0xcbf29ce484222325u64;
    for app in apps {
        for part in [
            &app.id,
            &app.component,
            app.label.as_deref().unwrap_or(""),
            app.search_component.as_deref().unwrap_or(""),
        ] {
            for byte in part.bytes().chain(std::iter::once(0)) {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
    }
    format!("{hash:016x}")
}

/// Legacy mutations participate in the same durable journal and physical-device
/// exclusion as the generic path. Read-only observations do not reserve a device.
pub struct LegacyGuard {
    inner: Arc<dyn crate::Control>,
    journal: Arc<dyn Journal>,
    clock: Arc<dyn Clock>,
    resources: BTreeMap<String, String>,
    max_budget_ms: u64,
}
impl LegacyGuard {
    pub fn new(
        inner: Arc<dyn crate::Control>,
        journal: Arc<dyn Journal>,
        clock: Arc<dyn Clock>,
        resources: BTreeMap<String, String>,
        max_budget_ms: u64,
    ) -> Self {
        Self {
            inner,
            journal,
            clock,
            resources,
            max_budget_ms,
        }
    }
}
impl crate::Control for LegacyGuard {
    fn execute(&self, req: tv_domain::Request, cancel: &dyn Cancellation) -> tv_domain::Receipt {
        if req.action == tv_domain::Action::Observe {
            return self.inner.execute(req, cancel);
        }
        let mut result = tv_domain::Receipt {
            request_id: req.id.clone(),
            status: Status::Invalid,
            code: "invalid_request",
            observed: None,
            cached: false,
            local_elapsed_ms: 0,
        };
        if !identifier(&req.id) || !identifier(&req.device) {
            return result;
        }
        let now = self.clock.unix_ms();
        if req.expires_at_unix_ms <= now {
            result.status = Status::Expired;
            result.code = "expired";
            return result;
        }
        if req.expires_at_unix_ms - now > self.max_budget_ms {
            result.code = "budget_exceeds_policy";
            return result;
        }
        let Some(resource) = self.resources.get(&req.device) else {
            result.code = "unknown_device";
            return result;
        };
        let action = match &req.action {
            tv_domain::Action::Observe => unreachable!(),
            tv_domain::Action::Pause => "pause".into(),
            tv_domain::Action::Resume => "resume".into(),
            tv_domain::Action::OpenApp { app } => format!("open_app:{}:{app}", app.len()),
            tv_domain::Action::SetVolume { index } => format!("set_volume:{index}"),
            tv_domain::Action::PlayItem {
                item_id,
                source_id,
                position_ms,
            } => format!(
                "play_item:{}:{item_id}:{}:{source_id}:{position_ms}",
                item_id.len(),
                source_id.len()
            ),
            tv_domain::Action::SeekMedia {
                item_id,
                source_id,
                position_ms,
            } => format!(
                "seek_media:{}:{item_id}:{}:{source_id}:{position_ms}",
                item_id.len(),
                source_id.len()
            ),
            tv_domain::Action::ResumeMedia { media } => {
                format!("resume_media:{}:{media}", media.len())
            }
        };
        let generic = Request {
            id: req.id.clone(),
            device: req.device.clone(),
            intent: Intent::Legacy { action },
            expires_at_unix_ms: req.expires_at_unix_ms,
        };
        match self.journal.reserve(&generic, resource, now) {
            Ok(Reservation::Acquired) => {}
            Ok(Reservation::Conflict) => {
                result.code = "request_id_conflict";
                return result;
            }
            Ok(Reservation::Existing(Operation::Complete(r))) => {
                result.status = r.status;
                result.code = if r.status == Status::OutcomeUnknown {
                    "outcome_unknown"
                } else {
                    "cached_operation_result"
                };
                result.observed = r.observed;
                result.cached = true;
                result.local_elapsed_ms = r.local_elapsed_ms;
                return result;
            }
            Ok(Reservation::Existing(_) | Reservation::Busy) => {
                result.status = Status::Busy;
                result.code = "operation_busy";
                return result;
            }
            Err(e) => {
                result.status = Status::Failed;
                result.code = e.code;
                return result;
            }
        }
        let combined = CombinedCancellation {
            outer: cancel,
            journal: &*self.journal,
            id: &generic.id,
        };
        result = self.inner.execute(req, &combined);
        let reply = Reply {
            request_id: result.request_id.clone(),
            status: result.status,
            code: result.code.into(),
            observed: result.observed.clone(),
            choices: vec![],
            cached: result.cached,
            local_elapsed_ms: result.local_elapsed_ms,
        };
        if self
            .journal
            .finish(&generic, &reply, self.clock.unix_ms())
            .is_err()
        {
            result.status = Status::OutcomeUnknown;
            result.code = "operation_persistence_failed";
        }
        result
    }
}

#[cfg(test)]
mod standby_failure_tests {
    use super::*;

    #[test]
    fn unsupported_standby_never_downgrades_dispatch_uncertainty() {
        let mut reply = Reply {
            request_id: "standby".into(),
            status: Status::Failed,
            code: String::new(),
            observed: None,
            choices: vec![],
            cached: false,
            local_elapsed_ms: 0,
        };
        fail(&mut reply, Failure::before("standby_unsupported"));
        assert_eq!(reply.status, Status::Unsupported);
        fail(&mut reply, Failure::uncertain("standby_unsupported"));
        assert_eq!(reply.status, Status::OutcomeUnknown);
    }
}
