//! Generic Android/Fire OS adapter. Discovery is data, never an authorization grant.
use crate::{
    adb::{media, parse_foreground, parse_playback, parse_volume},
    adb_server::ServerProbe,
    config::{Config, Device as ConfigDevice},
    process::Process,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tv_application::{generic::Device, Context};
use tv_domain::{generic::*, Failure, Observed, PlaybackEvidence, PortResult};

mod cache;
mod catalogue;
mod interaction;
mod observation;
mod screenshot;
#[cfg(test)]
mod tests;
mod transport;
mod ui;
use cache::valid_label;
#[cfg(test)]
use catalogue::parse_activities;
use catalogue::{parse_candidates, valid_component, Candidate};
use screenshot::validate_png;
use transport::ProtocolCache;
use ui::parse_ui;

const MAX_APPS: usize = 256;
const MAX_UI_NODES: usize = 1024;
const MAX_UI_ELEMENTS: usize = 64;
const REFERENCE_TTL_MS: u64 = 30_000;
// Power dumps contain vendor diagnostic histories, not just the current state.
// Keep this internal read separate from the ordinary text/MCP response budget.
const MAX_POWER_DUMP_BYTES: usize = 1024 * 1024;

pub struct GenericAdb {
    config: Arc<Config>,
    process: Arc<dyn Process>,
    probe: Arc<dyn ServerProbe>,
    protocol: Mutex<ProtocolCache>,
    views: Mutex<BTreeMap<String, IssuedView>>,
    session: u128,
    view_sequence: AtomicU64,
    cache_dir: Option<PathBuf>,
    labels: Mutex<BTreeMap<String, String>>,
    scan_cursor: Mutex<BTreeMap<String, usize>>,
}
struct IssuedView {
    snapshot: UiSnapshot,
    at: u64,
}
impl GenericAdb {
    pub fn new(
        config: Arc<Config>,
        process: Arc<dyn Process>,
        probe: Arc<dyn ServerProbe>,
    ) -> Self {
        Self {
            config,
            process,
            probe,
            protocol: Mutex::new(None),
            views: Mutex::new(BTreeMap::new()),
            view_sequence: AtomicU64::new(0),
            cache_dir: None,
            labels: Mutex::new(BTreeMap::new()),
            scan_cursor: Mutex::new(BTreeMap::new()),
            session: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                ^ u128::from(std::process::id()),
        }
    }
    pub fn with_cache_dir(mut self, path: PathBuf) -> Self {
        self.cache_dir = Some(path);
        self
    }
    fn foreground(&self, alias: &str, ctx: &Context<'_>) -> PortResult<Option<String>> {
        parse_foreground(&self.shell(
            alias,
            &["dumpsys", "activity", "activities"],
            &[],
            false,
            ctx,
        )?)
    }
    fn query(
        &self,
        alias: &str,
        action: &str,
        category: Option<&str>,
        ctx: &Context<'_>,
    ) -> PortResult<Vec<Candidate>> {
        let mut args = vec![
            "cmd",
            "package",
            "query-activities",
            "--user",
            "current",
            "-a",
            action,
        ];
        if let Some(category) = category {
            args.extend(["-c", category]);
        }
        let output = self.shell_bytes(alias, &args, &[], false, 256 * 1024, ctx)?;
        let output = String::from_utf8(output)
            .map_err(|_| Failure::before("app_discovery_output_invalid"))?;
        parse_candidates(&output)
    }
    fn launch(
        &self,
        alias: &str,
        app: &App,
        query: Option<&str>,
        ctx: &Context<'_>,
    ) -> PortResult<()> {
        let component = match query {
            Some(_) => app.search_component.as_deref().unwrap_or(&app.component),
            None => &app.component,
        };
        if !valid_component(component) || component.split('/').next() != Some(app.id.as_str()) {
            return Err(Failure::before("invalid_app_component"));
        }
        let mut args = vec!["am", "start", "--user", "current", "-W", "-n", component];
        if let Some(query) = query {
            if query.is_empty() || query.len() > 512 || query.chars().any(char::is_control) {
                return Err(Failure::before("invalid_search_query"));
            }
            args.extend(["-a", "android.intent.action.SEARCH", "--es", "query", query]);
        } else {
            args.extend(["-a", "android.intent.action.MAIN"]);
        }
        let output = self.shell(alias, &args, &[], true, ctx)?;
        if output.lines().any(|line| {
            let s = line.trim_start();
            s.starts_with("Error:")
                || s.starts_with("Exception")
                || s.starts_with("SecurityException")
        }) {
            return Err(Failure::uncertain("app_launch_not_confirmed"));
        }
        Ok(())
    }
    fn open_url(&self, alias: &str, app: &App, url: &str, ctx: &Context<'_>) -> PortResult<()> {
        // Restrict resolution and launch to the discovered app, not a global browser chooser.
        let raw = self.shell(
            alias,
            &[
                "cmd",
                "package",
                "query-activities",
                "--user",
                "current",
                "-a",
                "android.intent.action.VIEW",
                "-d",
                url,
                "-p",
                &app.id,
            ],
            &[],
            false,
            ctx,
        )?;
        let handlers = parse_candidates(&raw)?;
        if !handlers.iter().any(|candidate| candidate.app.id == app.id) {
            return Err(Failure::before("url_unsupported"));
        }
        let output = self.shell(
            alias,
            &[
                "am",
                "start",
                "--user",
                "current",
                "-W",
                "-a",
                "android.intent.action.VIEW",
                "-d",
                url,
                "-p",
                &app.id,
            ],
            &[],
            true,
            ctx,
        )?;
        if output.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("Error:")
                || line.starts_with("Exception")
                || line.starts_with("SecurityException")
        }) {
            return Err(Failure::uncertain("app_launch_not_confirmed"));
        }
        Ok(())
    }
    fn media_dump(&self, alias: &str, ctx: &Context<'_>) -> PortResult<String> {
        let bytes = self.shell_bytes(
            alias,
            &["dumpsys", "media_session"],
            &[],
            false,
            256 * 1024,
            ctx,
        )?;
        String::from_utf8(bytes).map_err(|_| Failure::before("media_state_unavailable"))
    }
    fn playback(&self, alias: &str, resume: bool, ctx: &Context<'_>) -> PortResult<()> {
        let package = self
            .foreground(alias, ctx)?
            .ok_or(Failure::before("foreground_unavailable"))?;
        let sessions = self.shell(
            alias,
            &["cmd", "media_session", "list-sessions"],
            &[],
            false,
            ctx,
        )?;
        let tag = session_tag(&sessions, &package)?;
        let state = media::snapshot(&self.media_dump(alias, ctx)?, &tag, &package)?;
        media::require_command(
            &state,
            &if resume {
                MediaCommand::Resume
            } else {
                MediaCommand::Pause
            },
        )?;
        // Session selection is explicit. Global key events could target another app.
        self.shell(
            alias,
            &["cmd", "media_session", "monitor", &tag],
            if resume { b"play\nq\n" } else { b"pause\nq\n" },
            true,
            ctx,
        )?;
        Ok(())
    }
}
impl GenericAdb {
    fn read_apps(
        &self,
        alias: &str,
        query: Option<&str>,
        ctx: &Context<'_>,
    ) -> PortResult<Vec<App>> {
        let mut apps = self.query(
            alias,
            "android.intent.action.MAIN",
            Some("android.intent.category.LEANBACK_LAUNCHER"),
            ctx,
        )?;
        apps.extend(self.query(
            alias,
            "android.intent.action.MAIN",
            Some("android.intent.category.LAUNCHER"),
            ctx,
        )?);
        apps.sort_by(|a, b| a.app.component.cmp(&b.app.component));
        apps.dedup_by(|a, b| a.app.component == b.app.component);
        if apps.len() > MAX_APPS {
            return Err(Failure::before("app_discovery_output_limit"));
        }
        let searches = match self.query(alias, "android.intent.action.SEARCH", None, ctx) {
            Ok(searches) => searches,
            Err(_) => {
                ctx.check()?;
                Vec::new()
            }
        };
        for candidate in &mut apps {
            let matches: Vec<_> = searches
                .iter()
                .filter(|search| search.app.id == candidate.app.id)
                .collect();
            if let [search] = matches.as_slice() {
                candidate.app.search_component = Some(search.app.component.clone());
            }
        }
        self.hydrate_labels(alias, &mut apps, query, ctx)?;
        Ok(apps.into_iter().map(|candidate| candidate.app).collect())
    }
}
impl Device for GenericAdb {
    fn resource(&self, alias: &str) -> PortResult<String> {
        self.target(alias)?;
        Ok(self.config.devices[alias].coordination_group().to_owned())
    }
    fn authorized(&self, alias: &str) -> PortResult<()> {
        self.target(alias)?;
        if !self.config.generic_adb_control {
            return Err(Failure::before("generic_control_not_authorized"));
        }
        Ok(())
    }
    fn apps(&self, alias: &str, ctx: &Context<'_>) -> PortResult<Vec<App>> {
        self.read_apps(alias, None, ctx)
    }
    fn apps_for(
        &self,
        alias: &str,
        query: Option<&str>,
        ctx: &Context<'_>,
    ) -> PortResult<Vec<App>> {
        self.read_apps(alias, query, ctx)
    }
    fn media(&self, alias: &str, ctx: &Context<'_>) -> PortResult<Vec<MediaSession>> {
        let listing = self.shell(
            alias,
            &["cmd", "media_session", "list-sessions"],
            &[],
            false,
            ctx,
        )?;
        let sessions = media::sessions(&listing)?;
        if sessions.is_empty() {
            return Ok(vec![]);
        }
        let raw = self.media_dump(alias, ctx)?;
        sessions
            .iter()
            .map(|(tag, package)| media::snapshot(&raw, tag, package))
            .collect()
    }
    fn observe_for(&self, alias: &str, intent: &Intent, ctx: &Context<'_>) -> PortResult<Observed> {
        self.observe_intent(alias, intent, ctx)
    }
    fn observe(&self, alias: &str, ctx: &Context<'_>) -> PortResult<Observed> {
        self.observe_state(alias, ctx)
    }
    fn screenshot(&self, alias: &str, ctx: &Context<'_>) -> PortResult<Screenshot> {
        let png = self
            .command_bytes(
                alias,
                "exec-out",
                &["screencap", "-p"],
                &[],
                false,
                8 * 1024 * 1024,
                ctx,
            )
            .map_err(|error| {
                if matches!(error.code, "adb_output_limit" | "process_output_limit") {
                    Failure::before("screenshot_too_large")
                } else {
                    error
                }
            })?;
        let (width, height) = validate_png(&png)?;
        ctx.check()?;
        Ok(Screenshot { png, width, height })
    }
    fn ui(&self, alias: &str, ctx: &Context<'_>) -> PortResult<UiSnapshot> {
        self.issue_ui(alias, ctx)
    }
    fn apply(
        &self,
        alias: &str,
        intent: &Intent,
        app: Option<&App>,
        ctx: &Context<'_>,
    ) -> PortResult<()> {
        self.authorized(alias)?;
        ctx.check()?;
        // Navigation or app changes invalidate previously issued references even
        // if the UI later returns to an identical-looking screen.
        if !matches!(intent, Intent::Activate { .. } | Intent::TypeText { .. }) {
            self.views
                .lock()
                .map_err(|_| Failure::before("adapter_state_unavailable"))?
                .remove(alias);
        }
        match intent {
            Intent::Legacy { .. } => Err(Failure::before("unsupported")),
            Intent::OpenApp { .. } => self.launch(
                alias,
                app.ok_or(Failure::before("app_not_resolved"))?,
                None,
                ctx,
            ),
            Intent::OpenUrl { url, .. } => self.open_url(
                alias,
                app.ok_or(Failure::before("app_not_resolved"))?,
                url,
                ctx,
            ),
            Intent::Search { query, .. } => self.launch(
                alias,
                app.ok_or(Failure::before("app_not_resolved"))?,
                Some(query),
                ctx,
            ),
            Intent::Wake => {
                // KEYCODE_WAKEUP is idempotent: never toggle an already-awake TV off.
                self.shell(alias, &["input", "keyevent", "224"], &[], true, ctx)?;
                Ok(())
            }
            Intent::SetVolume { index } => {
                // Recheck the native range immediately before dispatch, including
                // direct adapter callers; never guess an Android volume scale.
                self.observe_for(alias, intent, ctx)?;
                self.shell(
                    alias,
                    &[
                        "cmd",
                        "media_session",
                        "volume",
                        "--stream",
                        "3",
                        "--set",
                        &index.to_string(),
                    ],
                    &[],
                    true,
                    ctx,
                )?;
                Ok(())
            }
            Intent::Mute => {
                // Absolute zero is idempotent. Never use KEYCODE_VOLUME_MUTE (toggle),
                // ADJUST_TOGGLE_MUTE, an unmute command, or a volume-up fallback.
                self.shell(
                    alias,
                    &[
                        "cmd",
                        "media_session",
                        "volume",
                        "--stream",
                        "3",
                        "--set",
                        "0",
                    ],
                    &[],
                    true,
                    ctx,
                )?;
                Ok(())
            }
            Intent::Media { session, command } => {
                let listing = self.shell(
                    alias,
                    &["cmd", "media_session", "list-sessions"],
                    &[],
                    false,
                    ctx,
                )?;
                let sessions = media::sessions(&listing)?;
                let (tag, package) = media::select(&sessions, session)?;
                let state = media::snapshot(&self.media_dump(alias, ctx)?, tag, package)?;
                media::require_command(&state, command)?;
                let input: &[u8] = match command {
                    MediaCommand::Pause => b"pause\nq\n",
                    MediaCommand::Resume => b"play\nq\n",
                    MediaCommand::Next => b"next\nq\n",
                    MediaCommand::Previous => b"previous\nq\n",
                };
                let output = self.shell(
                    alias,
                    &["cmd", "media_session", "monitor", tag],
                    input,
                    true,
                    ctx,
                )?;
                // Exit status zero is not proof the command reached the session.
                if !output.contains(&format!("Monitoring session {tag}..."))
                    || output.contains("No session found")
                    || output.contains("Error")
                    || output.contains("Invalid command")
                {
                    return Err(Failure::uncertain("media_dispatch_unconfirmed"));
                }
                Ok(())
            }
            Intent::Pause => self.playback(alias, false, ctx),
            Intent::Resume => self.playback(alias, true, ctx),
            Intent::Navigate { keys } => {
                if keys.is_empty() || keys.len() > 16 {
                    return Err(Failure::before("invalid_navigation"));
                }
                let mut args = vec!["input", "keyevent"];
                args.extend(keys.iter().map(|key| match key {
                    Key::Up => "19",
                    Key::Down => "20",
                    Key::Left => "21",
                    Key::Right => "22",
                    Key::Select => "23",
                    Key::Back => "4",
                    Key::Home => "3",
                }));
                self.shell(alias, &args, &[], true, ctx)?;
                Ok(())
            }
            Intent::Activate { reference } | Intent::TypeText { reference, .. } => {
                self.interact(alias, intent, reference, ctx)
            }
        }
    }
}

fn session_tag(raw: &str, package: &str) -> PortResult<String> {
    let sessions = media::sessions(raw)?;
    media::unique_for_package(&sessions, package).map(str::to_owned)
}
