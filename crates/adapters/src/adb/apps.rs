//! Bounded, read-only discovery of the current Android user's TV launchers.
use super::Adb;
use crate::config::package;
use std::collections::BTreeSet;
use tv_application::Context;
use tv_domain::{Failure, PortResult};

const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_COMPONENT_BYTES: usize = 512;
const MAX_COMPONENTS: usize = 1024;
const MAX_PACKAGES: usize = 256;

impl Adb<'_> {
    /// Return sorted, unique package IDs with a MAIN/LEANBACK_LAUNCHER activity.
    /// This does not launch apps or add them to the configured app allowlist.
    /// Malformed, oversized or incomplete output fails without partial results.
    pub fn discover_apps(&self, alias: &str, ctx: &Context<'_>) -> PortResult<Vec<String>> {
        ctx.check()?;
        // AOSP PackageManagerShellCommand's --components mode omits all headers
        // and prints flattened component names, or exactly "No activities found".
        // https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/services/core/java/com/android/server/pm/PackageManagerShellCommand.java
        let output = self.shell(
            alias,
            &[
                "cmd",
                "package",
                "query-activities",
                "--components",
                "--user",
                "current",
                "-a",
                "android.intent.action.MAIN",
                "-c",
                "android.intent.category.LEANBACK_LAUNCHER",
            ],
            &[],
            false,
            ctx,
        )?;
        parse_apps(&output, self.config.preferences.max_output_bytes, ctx)
    }
}

fn parse_apps(output: &str, byte_limit: usize, ctx: &Context<'_>) -> PortResult<Vec<String>> {
    ctx.check()?;
    if output.len() > byte_limit.min(MAX_OUTPUT_BYTES) {
        return Err(Failure::before("app_discovery_output_limit"));
    }
    let mut packages = BTreeSet::new();
    let mut no_activities = false;
    let mut saw_line = false;
    for (index, line) in output.lines().enumerate() {
        ctx.check()?;
        if index >= MAX_COMPONENTS || line.len() > MAX_COMPONENT_BYTES {
            return Err(Failure::before("app_discovery_output_limit"));
        }
        // Empty output, error text, blank rows and mixed no-result/component
        // replies cannot be mistaken for a successfully discovered allowlist.
        if line == "No activities found" && !saw_line {
            no_activities = true;
        } else {
            if no_activities {
                return Err(Failure::before("app_discovery_output_invalid"));
            }
            let (package_id, class) = line
                .split_once('/')
                .ok_or(Failure::before("app_discovery_output_invalid"))?;
            if !package(package_id) || !valid_class(class) {
                return Err(Failure::before("app_discovery_output_invalid"));
            }
            packages.insert(package_id.to_owned());
            if packages.len() > MAX_PACKAGES {
                return Err(Failure::before("app_discovery_output_limit"));
            }
        }
        saw_line = true;
    }
    if !saw_line {
        return Err(Failure::before("app_discovery_output_invalid"));
    }
    ctx.check()?;
    Ok(packages.into_iter().collect())
}

fn valid_class(class: &str) -> bool {
    class
        .strip_prefix('.')
        .unwrap_or(class)
        .split('.')
        .all(|part| {
            !part.is_empty()
                && (part.as_bytes()[0].is_ascii_alphabetic() || b"_$".contains(&part.as_bytes()[0]))
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_$".contains(&b))
        })
}

#[cfg(test)]
mod tests;
