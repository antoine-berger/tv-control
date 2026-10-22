use crate::{
    config::{Config, Device},
    process::{Invocation, Process},
};
use std::collections::BTreeMap;
use tv_application::Context;
use tv_domain::*;

pub struct Adb<'a> {
    config: &'a Config,
    process: &'a dyn Process,
    probe: &'a dyn crate::adb_server::ServerProbe,
}
impl<'a> Adb<'a> {
    pub fn new(
        config: &'a Config,
        process: &'a dyn Process,
        probe: &'a dyn crate::adb_server::ServerProbe,
    ) -> Self {
        Self {
            config,
            process,
            probe,
        }
    }
    fn call(
        &self,
        alias: &str,
        args: &[&str],
        input: &[u8],
        mutating: bool,
        ctx: &Context<'_>,
    ) -> PortResult<String> {
        let Some(Device::Adb {
            endpoint,
            server_host,
            server_port,
            ..
        }) = self.config.devices.get(alias)
        else {
            return Err(Failure::before("unsupported"));
        };
        // `adb version` is local-only. A manager-capable client must not reach a
        // known incompatible server, because AOSP may issue host:kill on mismatch.
        let version = self.process.run(
            Invocation {
                program: self.config.executables.adb.clone(),
                args: vec!["version".into()],
                env: BTreeMap::new(),
                stdin: Vec::new(),
                mutating: false,
            },
            ctx,
        )?;
        let client_protocol = std::str::from_utf8(&version.stdout)
            .ok()
            .and_then(|s| {
                s.lines().find_map(|line| {
                    line.strip_prefix("Android Debug Bridge version 1.0.")
                        .and_then(|s| s.trim().parse::<u32>().ok())
                })
            })
            .filter(|_| version.success)
            .ok_or(Failure::before("adb_client_protocol_unavailable"))?;
        if self.probe.version(server_host, *server_port, ctx)? != client_protocol {
            return Err(Failure::before("adb_server_protocol_mismatch"));
        }
        let mut arguments: Vec<std::ffi::OsString> = [
            "-H",
            server_host,
            "-P",
            &server_port.to_string(),
            "-s",
            endpoint,
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        arguments.extend(args.iter().map(|s| std::ffi::OsString::from(*s)));
        let output = self.process.run(
            Invocation {
                program: self.config.executables.adb.clone(),
                args: arguments,
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
        if !output.success {
            return Err(error());
        }
        String::from_utf8(output.stdout).map_err(|_| error())
    }
    fn shell(
        &self,
        alias: &str,
        args: &[&str],
        input: &[u8],
        mutating: bool,
        ctx: &Context<'_>,
    ) -> PortResult<String> {
        // Remote shell syntax is constructed only from constants and validated configuration values.
        let command = args
            .iter()
            .map(|s| format!("'{}'", s.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ");
        self.call(alias, &["shell", &command], input, mutating, ctx)
    }
    pub fn observe(&self, alias: &str, ctx: &Context<'_>) -> PortResult<Observed> {
        let state = self.call(alias, &["get-state"], &[], false, ctx)?;
        if state.trim() != "device" {
            return Ok(Observed::default());
        }
        let activities = self.shell(
            alias,
            &["dumpsys", "activity", "activities"],
            &[],
            false,
            ctx,
        )?;
        let foreground = parse_foreground(&activities)?;
        let active_app = self.config.apps.iter().find_map(|(name, app)| {
            (foreground.as_ref() == Some(&app.package)).then(|| name.clone())
        });
        let Some(Device::Adb {
            media_app,
            volume_max_index,
            ..
        }) = self.config.devices.get(alias)
        else {
            return Err(Failure::before("unsupported"));
        };
        let mut observed = Observed {
            connected: true,
            active_app,
            playback_evidence: PlaybackEvidence::NativeState,
            ..Observed::default()
        };
        if let Some(app) = media_app {
            let raw = self.shell(alias, &["dumpsys", "media_session"], &[], false, ctx)?;
            observed.playback = parse_playback(&raw, &self.config.apps[app].package)?;
        }
        if let Some(max) = volume_max_index {
            let raw = self.shell(
                alias,
                &["cmd", "media_session", "volume", "--stream", "3", "--get"],
                &[],
                false,
                ctx,
            )?;
            let (index, observed_max) =
                parse_volume(&raw).ok_or(Failure::before("volume_state_unavailable"))?;
            if observed_max != *max {
                return Err(Failure::before("volume_calibration_changed"));
            }
            observed.volume_index = Some(index);
        }
        Ok(observed)
    }
    pub fn apply(&self, alias: &str, action: &Action, ctx: &Context<'_>) -> PortResult<()> {
        let Some(Device::Adb {
            media_app,
            volume_max_index,
            ..
        }) = self.config.devices.get(alias)
        else {
            return Err(Failure::before("unsupported"));
        };
        match action {
            Action::OpenApp { app } => {
                let package = &self
                    .config
                    .apps
                    .get(app)
                    .ok_or(Failure::before("unknown_app"))?
                    .package;
                let output = self.shell(
                    alias,
                    &[
                        "cmd",
                        "package",
                        "resolve-activity",
                        "--brief",
                        "-a",
                        "android.intent.action.MAIN",
                        "-c",
                        "android.intent.category.LEANBACK_LAUNCHER",
                        "-p",
                        package,
                    ],
                    &[],
                    false,
                    ctx,
                )?;
                let component = output.lines().last().unwrap_or("").trim();
                if !component.starts_with(&format!("{package}/"))
                    || !component
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._$/".contains(&b))
                {
                    return Err(Failure::before("app_not_launchable"));
                }
                let output = self.shell(
                    alias,
                    &["am", "start", "-W", "-n", component],
                    &[],
                    true,
                    ctx,
                )?;
                if output
                    .lines()
                    .any(|s| s.starts_with("Error:") || s.starts_with("Exception"))
                {
                    return Err(Failure::uncertain("app_launch_not_confirmed"));
                }
            }
            Action::Pause | Action::Resume => {
                let app = media_app.as_ref().ok_or(Failure::before("unsupported"))?;
                let package = &self.config.apps[app].package;
                let output = self.shell(
                    alias,
                    &["cmd", "media_session", "list-sessions"],
                    &[],
                    false,
                    ctx,
                )?;
                let all_tags: Vec<_> = output
                    .lines()
                    .filter_map(|line| {
                        let (tag, pkg) =
                            line.trim().strip_prefix("tag=")?.split_once(", package=")?;
                        (!tag.is_empty()
                            && tag.len() <= 128
                            && tag
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b)))
                        .then_some((tag, pkg.trim()))
                    })
                    .collect();
                let mut tags = all_tags
                    .iter()
                    .filter_map(|(tag, pkg)| (*pkg == package).then_some(*tag));
                let tag = tags
                    .next()
                    .ok_or(Failure::before("media_session_not_found"))?;
                if tags.next().is_some() {
                    return Err(Failure::before("media_session_ambiguous"));
                }
                if all_tags.iter().filter(|(other, _)| *other == tag).count() != 1 {
                    return Err(Failure::before("media_tag_ambiguous"));
                }
                let command = if *action == Action::Pause {
                    b"pause\nq\n".as_slice()
                } else {
                    b"play\nq\n".as_slice()
                };
                self.shell(
                    alias,
                    &["cmd", "media_session", "monitor", tag],
                    command,
                    true,
                    ctx,
                )?;
            }
            Action::SetVolume { index } => {
                let max = volume_max_index.ok_or(Failure::before("unsupported"))?;
                if *index > max {
                    return Err(Failure::before("volume_out_of_range"));
                }
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
            }
            _ => return Err(Failure::before("unsupported")),
        }
        Ok(())
    }
}
pub fn parse_playback(raw: &str, package: &str) -> PortResult<Playback> {
    let mut current = false;
    let mut states = Vec::new();
    for line in raw.lines() {
        if let Some(value) = line.trim().strip_prefix("package=") {
            current = value.split_whitespace().next() == Some(package);
        }
        if current {
            if let Some((_, state)) = line.split_once("state=PlaybackState {state=") {
                states.push(match state.split(|c: char| !c.is_ascii_digit()).next() {
                    Some("2") => Playback::Paused,
                    Some("3")
                        if state
                            .split_once("speed=")
                            .and_then(|(_, s)| {
                                s.split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
                                    .next()
                            })
                            .and_then(|s| s.parse::<f32>().ok())
                            .is_some_and(|s| s > 0.0) =>
                    {
                        Playback::Playing
                    }
                    Some("6") => Playback::Buffering,
                    Some("0" | "1") => Playback::Stopped,
                    _ => Playback::Unknown,
                });
                current = false;
            }
        }
    }
    match states.as_slice() {
        [] => Ok(Playback::Unknown),
        [state] => Ok(*state),
        _ => Err(Failure::before("media_session_ambiguous")),
    }
}
pub fn parse_foreground(raw: &str) -> PortResult<Option<String>> {
    let mut packages = std::collections::BTreeSet::new();
    for line in raw
        .lines()
        .filter(|line| line.contains("topResumedActivity") || line.contains("mResumedActivity"))
    {
        for token in line.split_whitespace() {
            if let Some((package, _)) = token.split_once('/') {
                if crate::config::package(package) {
                    packages.insert(package.to_string());
                }
            }
        }
    }
    if packages.len() > 1 {
        return Err(Failure::before("foreground_ambiguous"));
    }
    Ok(packages.into_iter().next())
}
pub fn parse_volume(raw: &str) -> Option<(u16, u16)> {
    let (_, rest) = raw.split_once("volume is ")?;
    let (index, range) = rest.split_once(" in range [0..")?;
    let max = range.split(']').next()?.parse::<u16>().ok()?;
    let index = index.parse::<u16>().ok()?;
    (index <= max).then_some((index, max))
}
