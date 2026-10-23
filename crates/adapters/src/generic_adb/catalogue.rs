//! ActivityInfo/ResolveInfo discovery parser. Labels come from observed metadata.
use super::{valid_label, MAX_APPS};
use crate::config;
use tv_domain::{generic::App, Failure, PortResult};
pub(super) fn valid_component(value: &str) -> bool {
    if value.len() > 512 {
        return false;
    }
    let Some((pkg, class)) = value.split_once('/') else {
        return false;
    };
    config::package(pkg)
        && class
            .strip_prefix('.')
            .unwrap_or(class)
            .split('.')
            .all(|part| {
                !part.is_empty()
                    && (part.as_bytes()[0].is_ascii_alphabetic()
                        || b"_$".contains(&part.as_bytes()[0]))
                    && part
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_$".contains(&c))
            })
}
#[derive(Default)]
struct Metadata {
    activity_resource: bool,
    name: String,
    package: String,
    label: Option<String>,
    resource: Option<u32>,
    source: Option<String>,
    version: Option<u64>,
}
pub(super) struct Candidate {
    pub(super) app: App,
    pub(super) resource: Option<u32>,
    pub(super) source: Option<String>,
    pub(super) version: Option<u64>,
}
#[cfg(test)]
pub(super) fn parse_activities(raw: &str) -> PortResult<Vec<App>> {
    Ok(parse_candidates(raw)?.into_iter().map(|c| c.app).collect())
}
pub(super) fn parse_candidates(raw: &str) -> PortResult<Vec<Candidate>> {
    if raw.len() > 256 * 1024 {
        return Err(Failure::before("app_discovery_output_limit"));
    }
    if raw.trim() == "No activities found" {
        return Ok(Vec::new());
    }
    let mut apps = Vec::new();
    let mut current: Option<Metadata> = None;
    let mut in_activity = false;
    let mut pending_label = None;
    let expected = raw
        .lines()
        .next()
        .and_then(|s| s.trim().strip_suffix(" activities found:"))
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or(Failure::before("app_discovery_output_invalid"))?;
    if expected > MAX_APPS {
        return Err(Failure::before("app_discovery_output_limit"));
    }
    let flush = |entry: Option<Metadata>, out: &mut Vec<Candidate>| -> PortResult<()> {
        if let Some(m) = entry {
            let component = format!("{}/{}", m.package, m.name);
            if !valid_component(&component) {
                return Err(Failure::before("app_discovery_output_invalid"));
            }
            out.push(Candidate {
                app: App {
                    id: m.package,
                    component,
                    label: m.label,
                    search_component: None,
                },
                resource: m.resource,
                source: m.source,
                version: m.version,
            });
            if out.len() > MAX_APPS {
                return Err(Failure::before("app_discovery_output_limit"));
            }
        }
        Ok(())
    };
    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with("Activity #") && line.ends_with(':') {
            flush(current.take(), &mut apps)?;
            pending_label = None;
            in_activity = false;
        } else if line == "ActivityInfo:" {
            flush(current.take(), &mut apps)?;
            current = Some(Metadata {
                label: pending_label.take(),
                ..Metadata::default()
            });
            in_activity = true;
        } else if line == "ApplicationInfo:" {
            if let Some(m) = &mut current {
                m.activity_resource = m.resource.is_some();
            }
            in_activity = false;
        } else if let Some(m) = &mut current {
            if in_activity {
                if let Some(value) = line.strip_prefix("name=") {
                    m.name = value.to_owned();
                }
                if let Some(value) = line.strip_prefix("packageName=") {
                    m.package = value.split_whitespace().next().unwrap_or("").to_owned();
                }
            } else {
                if let Some(value) = line.strip_prefix("sourceDir=") {
                    if apk_path(value) {
                        m.source = Some(value.to_owned());
                    }
                }
                if let Some(value) = line
                    .split_whitespace()
                    .find_map(|part| part.strip_prefix("versionCode="))
                {
                    m.version = value.parse().ok();
                }
            }
            if let Some(value) = line
                .strip_prefix("labelRes=0x")
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| u32::from_str_radix(s, 16).ok())
                .filter(|v| *v != 0)
            {
                if m.resource.is_none() {
                    m.resource = Some(value);
                }
            }
            if let Some((_, text)) = line.split_once("nonLocalizedLabel=") {
                let value = text
                    .rsplit_once(" icon=0x")
                    .map(|(label, _)| label)
                    .unwrap_or(text)
                    .trim();
                if (in_activity || !m.activity_resource)
                    && m.label.is_none()
                    && value != "null"
                    && !value.is_empty()
                    && value.len() <= 256
                    && !value.chars().any(char::is_control)
                {
                    m.label = Some(value.to_owned());
                }
            }
        } else if let Some((_, text)) = line.split_once("nonLocalizedLabel=") {
            let label = text
                .rsplit_once(" icon=0x")
                .map(|(label, _)| label)
                .unwrap_or(text)
                .trim();
            if label != "null" && valid_label(label) {
                pending_label = Some(label.to_owned());
            }
        }
    }
    flush(current, &mut apps)?;
    if apps.is_empty() || apps.len() != expected {
        return Err(Failure::before("app_discovery_output_invalid"));
    }
    Ok(apps)
}
fn apk_path(path: &str) -> bool {
    path.len() <= 1024
        && path.starts_with('/')
        && path.ends_with(".apk")
        && !path.split('/').any(|p| p == ".." || p == ".")
        && !path.chars().any(char::is_control)
}
