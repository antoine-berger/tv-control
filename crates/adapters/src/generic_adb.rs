//! Generic Android/Fire OS adapter. Discovery is data, never an authorization grant.
use crate::{
    adb::{parse_foreground, parse_playback, parse_volume},
    adb_server::ServerProbe,
    config::{Config, Device as ConfigDevice},
    process::{Invocation, Process},
};
use std::{
    collections::BTreeMap,
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tv_application::{generic::Device, Context};
use tv_domain::{generic::*, Failure, Observed, PlaybackEvidence, PortResult};

mod cache;
mod catalogue;
mod screenshot;
#[cfg(test)]
mod tests;
mod ui;
use cache::valid_label;
#[cfg(test)]
use catalogue::parse_activities;
use catalogue::{parse_candidates, valid_component, Candidate};
use screenshot::validate_png;
use ui::parse_ui;

const MAX_APPS: usize = 256;
const MAX_UI_NODES: usize = 1024;
const MAX_UI_ELEMENTS: usize = 64;
const REFERENCE_TTL_MS: u64 = 30_000;

#[derive(Clone, Copy, PartialEq, Eq)]
struct ExecutableStamp {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
type ProtocolCache = Option<(Option<ExecutableStamp>, u32)>;

pub struct GenericAdb {
    config: Arc<Config>,
    process: Arc<dyn Process>,
    probe: Arc<dyn ServerProbe>,
    protocol: Mutex<ProtocolCache>,
    views: Mutex<BTreeMap<String, IssuedView>>,
    session: u128,
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
    fn target(&self, alias: &str) -> PortResult<(&str, &str, u16)> {
        match self.config.devices.get(alias) {
            Some(ConfigDevice::Adb {
                endpoint,
                server_host,
                server_port,
                ..
            }) => Ok((endpoint, server_host, *server_port)),
            Some(_) => Err(Failure::before("unsupported")),
            None => Err(Failure::before("unknown_device")),
        }
    }
    fn protocol(&self, ctx: &Context<'_>) -> PortResult<u32> {
        ctx.check()?;
        let stamp = std::fs::metadata(&self.config.executables.adb)
            .ok()
            .map(|m| ExecutableStamp {
                device: m.dev(),
                inode: m.ino(),
                size: m.len(),
                modified: (m.mtime(), m.mtime_nsec()),
                changed: (m.ctime(), m.ctime_nsec()),
            });
        let mut cache = self
            .protocol
            .try_lock()
            .map_err(|_| Failure::before("adapter_state_unavailable"))?;
        if let Some((old, version)) = *cache {
            if old == stamp {
                return Ok(version);
            }
        }
        let result = self.process.run(
            Invocation {
                program: self.config.executables.adb.clone(),
                args: vec!["version".into()],
                env: BTreeMap::new(),
                stdin: Vec::new(),
                mutating: false,
            },
            ctx,
        )?;
        let version = std::str::from_utf8(&result.stdout)
            .ok()
            .filter(|_| result.success)
            .and_then(|s| {
                s.lines().find_map(|line| {
                    line.strip_prefix("Android Debug Bridge version 1.0.")?
                        .trim()
                        .parse::<u32>()
                        .ok()
                })
            })
            .ok_or(Failure::before("adb_client_protocol_unavailable"))?;
        ctx.check()?;
        *cache = Some((stamp, version));
        Ok(version)
    }
    fn shell(
        &self,
        alias: &str,
        args: &[&str],
        input: &[u8],
        mutating: bool,
        ctx: &Context<'_>,
    ) -> PortResult<String> {
        let output = self.shell_bytes(
            alias,
            args,
            input,
            mutating,
            self.config.preferences.max_output_bytes,
            ctx,
        )?;
        String::from_utf8(output).map_err(|_| {
            if mutating {
                Failure::uncertain("adb_command_failed")
            } else {
                Failure::before("adb_read_failed")
            }
        })
    }
    fn shell_bytes(
        &self,
        alias: &str,
        args: &[&str],
        input: &[u8],
        mutating: bool,
        limit: usize,
        ctx: &Context<'_>,
    ) -> PortResult<Vec<u8>> {
        self.command_bytes(alias, "shell", args, input, mutating, limit, ctx)
    }
    #[allow(clippy::too_many_arguments)]
    fn command_bytes(
        &self,
        alias: &str,
        transport: &str,
        args: &[&str],
        input: &[u8],
        mutating: bool,
        limit: usize,
        ctx: &Context<'_>,
    ) -> PortResult<Vec<u8>> {
        let (endpoint, host, port) = self.target(alias)?;
        let protocol = self.protocol(ctx)?;
        // Probe the existing server on every call. No manager-capable client is
        // allowed to contact an incompatible server, even when its version is cached.
        if self.probe.version(host, port, ctx)? != protocol {
            return Err(Failure::before("adb_server_protocol_mismatch"));
        }
        ctx.check()?;
        let command = args
            .iter()
            .map(|s| format!("'{}'", s.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ");
        let result = self.process.run(
            Invocation {
                program: self.config.executables.adb.clone(),
                args: vec![
                    "-H".into(),
                    host.into(),
                    "-P".into(),
                    port.to_string().into(),
                    "-s".into(),
                    endpoint.into(),
                    transport.into(),
                    command.into(),
                ],
                env: BTreeMap::from([
                    ("ADB_MDNS".into(), "0".into()),
                    ("ADB_USB".into(), "0".into()),
                ]),
                stdin: input.to_vec(),
                mutating,
            },
            ctx,
        )?;
        let error = || {
            if mutating {
                Failure::uncertain("adb_command_failed")
            } else {
                Failure::before("adb_read_failed")
            }
        };
        if !result.success || result.stdout.len() > limit {
            return Err(error());
        }
        let output = result.stdout;
        ctx.check().map_err(|e| {
            if mutating {
                Failure::uncertain(e.code)
            } else {
                e
            }
        })?;
        Ok(output)
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
    fn read_ui(&self, alias: &str, ctx: &Context<'_>) -> PortResult<UiSnapshot> {
        let raw = self.shell_bytes(
            alias,
            &["uiautomator", "dump", "--compressed", "/proc/self/fd/1"],
            &[],
            false,
            512 * 1024,
            ctx,
        )?;
        let raw = String::from_utf8(raw).map_err(|_| Failure::before("ui_unavailable"))?;
        parse_ui(alias, self.session, &raw)
    }
    fn launch(
        &self,
        alias: &str,
        app: &App,
        query: Option<&str>,
        ctx: &Context<'_>,
    ) -> PortResult<()> {
        let component = match query {
            Some(_) => app
                .search_component
                .as_deref()
                .ok_or(Failure::before("search_unsupported"))?,
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
    fn observe_for(&self, alias: &str, intent: &Intent, ctx: &Context<'_>) -> PortResult<Observed> {
        let package = self.foreground(alias, ctx)?;
        let mut observed = Observed {
            connected: true,
            active_app: package.clone(),
            playback_evidence: PlaybackEvidence::NativeState,
            ..Observed::default()
        };
        if matches!(intent, Intent::Pause | Intent::Resume) {
            if let Some(package) = package {
                let raw = self.shell(alias, &["dumpsys", "media_session"], &[], false, ctx)?;
                observed.playback = parse_playback(&raw, &package)?;
            }
        }
        Ok(observed)
    }
    fn observe(&self, alias: &str, ctx: &Context<'_>) -> PortResult<Observed> {
        // A successful targeted shell observation already proves transport connectivity.
        let package = self.foreground(alias, ctx)?;
        let mut observed = Observed {
            connected: true,
            active_app: package.clone(),
            playback_evidence: PlaybackEvidence::NativeState,
            ..Observed::default()
        };
        if let Some(package) = package {
            let raw = self.shell(alias, &["dumpsys", "media_session"], &[], false, ctx)?;
            observed.playback = parse_playback(&raw, &package)?;
        }
        if let Some(ConfigDevice::Adb {
            volume_max_index: Some(max),
            ..
        }) = self.config.devices.get(alias)
        {
            let raw = self.shell(
                alias,
                &["cmd", "media_session", "volume", "--stream", "3", "--get"],
                &[],
                false,
                ctx,
            )?;
            let (index, actual_max) =
                parse_volume(&raw).ok_or(Failure::before("volume_state_unavailable"))?;
            if actual_max != *max {
                return Err(Failure::before("volume_calibration_changed"));
            }
            observed.volume_index = Some(index);
        }
        Ok(observed)
    }
    fn screenshot(&self, alias: &str, ctx: &Context<'_>) -> PortResult<Screenshot> {
        let png = self.command_bytes(
            alias,
            "exec-out",
            &["screencap", "-p"],
            &[],
            false,
            1024 * 1024,
            ctx,
        )?;
        let (width, height) = validate_png(&png)?;
        ctx.check()?;
        Ok(Screenshot { png, width, height })
    }
    fn ui(&self, alias: &str, ctx: &Context<'_>) -> PortResult<UiSnapshot> {
        let snapshot = self.read_ui(alias, ctx)?;
        self.views
            .lock()
            .map_err(|_| Failure::before("adapter_state_unavailable"))?
            .insert(
                alias.to_owned(),
                IssuedView {
                    snapshot: snapshot.clone(),
                    at: ctx.clock.monotonic_ms(),
                },
            );
        Ok(snapshot)
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
        match intent {
            Intent::Legacy { .. } => Err(Failure::before("unsupported")),
            Intent::OpenApp { .. } => self.launch(
                alias,
                app.ok_or(Failure::before("app_not_resolved"))?,
                None,
                ctx,
            ),
            Intent::Search { query, .. } => self.launch(
                alias,
                app.ok_or(Failure::before("app_not_resolved"))?,
                Some(query),
                ctx,
            ),
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
            Intent::Activate { reference } => {
                let (snapshot, at) = {
                    let views = self
                        .views
                        .lock()
                        .map_err(|_| Failure::before("adapter_state_unavailable"))?;
                    let view = views
                        .get(alias)
                        .ok_or(Failure::before("ui_reference_unknown"))?;
                    (view.snapshot.clone(), view.at)
                };
                if ctx.clock.monotonic_ms().saturating_sub(at) > REFERENCE_TTL_MS {
                    return Err(Failure::before("ui_reference_expired"));
                }
                let element = snapshot
                    .elements
                    .iter()
                    .find(|e| &e.reference == reference)
                    .ok_or(Failure::before("ui_reference_unknown"))?;
                let fresh = self.read_ui(alias, ctx)?;
                if fresh != snapshot {
                    return Err(Failure::before("ui_reference_stale"));
                }
                // Consume references before dispatch; even an uncertain tap cannot be replayed with them.
                self.views
                    .lock()
                    .map_err(|_| Failure::before("adapter_state_unavailable"))?
                    .remove(alias);
                let [left, top, right, bottom] = element.bounds;
                let x = left + (right - left) / 2;
                let y = top + (bottom - top) / 2;
                self.shell(
                    alias,
                    &["input", "tap", &x.to_string(), &y.to_string()],
                    &[],
                    true,
                    ctx,
                )?;
                Ok(())
            }
        }
    }
}

fn session_tag(raw: &str, package: &str) -> PortResult<String> {
    let tags: Vec<_> = raw
        .lines()
        .filter_map(|line| {
            let (tag, pkg) = line.trim().strip_prefix("tag=")?.split_once(", package=")?;
            (!tag.is_empty()
                && tag.len() <= 128
                && tag
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b)))
            .then_some((tag, pkg.trim()))
        })
        .collect();
    let found: Vec<_> = tags.iter().filter(|(_, p)| *p == package).collect();
    let [(tag, _)] = found.as_slice() else {
        return Err(Failure::before(if found.is_empty() {
            "media_session_not_found"
        } else {
            "media_session_ambiguous"
        }));
    };
    if tags.iter().filter(|(other, _)| other == tag).count() != 1 {
        return Err(Failure::before("media_tag_ambiguous"));
    }
    Ok((*tag).to_owned())
}
