//! Fresh read-only package evidence. Never execute an intent or cache app capabilities.
use super::{valid_component, App, Context, Failure, GenericAdb, PortResult};
use tv_domain::generic::{AppDetails, AppVersion, NativeSearch};

impl GenericAdb {
    pub(super) fn read_app_details(
        &self,
        alias: &str,
        app: &App,
        ctx: &Context<'_>,
    ) -> PortResult<AppDetails> {
        ctx.check()?;
        if !valid_component(&app.component)
            || app.component.split('/').next() != Some(app.id.as_str())
        {
            return Err(Failure::before("invalid_app_component"));
        }
        let dump = self.detail_read(alias, &["dumpsys", "package", &app.id], 256 * 1024, ctx)?;
        let version = dump.as_deref().and_then(|raw| active_version(raw, &app.id));
        let resolved = self.detail_read(
            alias,
            &[
                "cmd",
                "package",
                "resolve-activity",
                "--brief",
                "--user",
                "current",
                "-a",
                "android.intent.action.SEARCH",
                "-p",
                &app.id,
            ],
            4096,
            ctx,
        )?;
        let native_search = resolved
            .as_deref()
            .map(|raw| search_handler(raw, &app.id))
            .unwrap_or(NativeSearch::Unknown);
        ctx.check()?;
        Ok(AppDetails {
            app: app.clone(),
            version,
            native_search,
        })
    }

    fn detail_read(
        &self,
        alias: &str,
        args: &[&str],
        limit: usize,
        ctx: &Context<'_>,
    ) -> PortResult<Option<String>> {
        match self.shell_bytes(alias, args, &[], false, limit, ctx) {
            Ok(bytes) => Ok(String::from_utf8(bytes).ok()),
            // Unsupported platform commands/permission denials aren't negative capability proof.
            Err(error) if error.code == "adb_read_failed" => {
                ctx.check()?;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

/// Only the exact package in the active Packages section is evidence. In particular,
/// Hidden system packages contains superseded factory versions on updated system apps.
fn active_version(raw: &str, package: &str) -> Option<AppVersion> {
    if raw.len() > 256 * 1024 {
        return None;
    }
    let mut active_section = false;
    let mut selected = false;
    let mut records = 0;
    let mut code = None;
    let mut name = None;
    let mut codes = 0;
    let mut names = 0;
    for line in raw.lines() {
        let value = line.trim();
        if !line.starts_with(char::is_whitespace) && !value.is_empty() {
            active_section = value == "Packages:";
            selected = false;
            continue;
        }
        if !active_section {
            continue;
        }
        if let Some(tail) = line.strip_prefix("  Package [") {
            selected = tail.split_once(']').is_some_and(|(id, suffix)| {
                id == package && suffix.starts_with(" (") && suffix.ends_with("):")
            });
            if selected {
                records += 1;
            }
            continue;
        }
        // A malformed/new peer or section cannot lend its version to this package.
        if !value.is_empty()
            && (line.len() - line.trim_start().len() <= 2 || value.starts_with("Package ["))
        {
            selected = false;
        }
        if !selected || !line.starts_with("    ") || line.starts_with("     ") {
            continue;
        }
        if let Some(tail) = value.strip_prefix("versionCode=") {
            codes += 1;
            code = tail
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u64>().ok());
        }
        if let Some(tail) = value.strip_prefix("versionName=") {
            names += 1;
            if tail.is_empty()
                || tail == "null"
                || tail.len() > 256
                || tail.chars().any(char::is_control)
            {
                return None;
            }
            name = Some(tail.to_owned());
        }
    }
    if records != 1 || codes != 1 || names > 1 {
        return None;
    }
    Some(AppVersion { code: code?, name })
}

fn search_handler(raw: &str, package: &str) -> NativeSearch {
    let lines: Vec<_> = raw
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if lines == ["No activity found"] {
        return NativeSearch::Unsupported;
    }
    let component = match lines.as_slice() {
        [component] => *component,
        [header, component] if resolution_header(header) => *component,
        _ => return NativeSearch::Unknown,
    };
    if valid_component(component) && component.split('/').next() == Some(package) {
        NativeSearch::Handler {
            component: component.to_owned(),
        }
    } else {
        NativeSearch::Unknown
    }
}
fn resolution_header(header: &str) -> bool {
    let parts: Vec<_> = header.split_whitespace().collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([
                "priority=",
                "preferredOrder=",
                "match=",
                "specificIndex=",
                "isDefault=",
            ])
            .all(|(part, prefix)| part.strip_prefix(prefix).is_some_and(|v| !v.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    const ACTIVE: &str = "Packages:\n  Package [org.example.player] (abc):\n    versionCode=530340 minSdk=21 targetSdk=34\n    versionName=5.30.340\n";
    #[test]
    fn active_package_version_excludes_other_and_hidden_system_versions() {
        let raw = format!("{ACTIVE}  Package [org.other.app] (def):\n    versionCode=777\n    versionName=other\nHidden system packages:\n  Package [org.example.player] (123):\n    versionCode=100\n    versionName=old\n");
        assert_eq!(
            active_version(&raw, "org.example.player"),
            Some(AppVersion {
                code: 530340,
                name: Some("5.30.340".into())
            })
        );
        assert_eq!(active_version(&raw, "org.other.app").unwrap().code, 777);
        assert_eq!(active_version(&raw, "org.missing.app"), None);
    }
    #[test]
    fn ambiguous_versions_and_non_active_records_are_unknown() {
        for raw in [
            ACTIVE.replace("Packages:", "Hidden system packages:"),
            "Packages:\n  Package [org.example.player] (abc):\n  Hidden system packages:\n    versionCode=1\n    versionName=old\n".into(),
            "Packages:\n  Package [org.example.player] (abc):\n   Package [org.other.app] (def):\n    versionCode=1\n    versionName=other\n".into(),
            format!("{ACTIVE}  Package [org.example.player] (def):\n    versionCode=111\n"),
            format!("{ACTIVE}    versionCode=111\n"),
            format!("{ACTIVE}    versionName=duplicate\n"),
            ACTIVE.replace("530340", "nonsense"),
            ACTIVE.replace("    versionCode", "      versionCode"),
            ACTIVE.replace("5.30.340", &"x".repeat(257)),
        ] {
            assert_eq!(active_version(&raw, "org.example.player"), None, "{raw}");
        }
    }
    #[test]
    fn search_resolution_is_scoped_and_never_uses_launcher_fallback() {
        assert_eq!(
            search_handler("No activity found\n", "org.example.player"),
            NativeSearch::Unsupported
        );
        for raw in [
            "",
            "Error: unknown command",
            "No activities found",
            "android/com.android.internal.app.ResolverActivity",
            "org.other.app/.Search",
            "org.example.player/.Search\norg.example.player/.Other",
        ] {
            assert_eq!(
                search_handler(raw, "org.example.player"),
                NativeSearch::Unknown
            );
        }
        for raw in ["org.example.player/.Search", "priority=0 preferredOrder=0 match=0x108000 specificIndex=-1 isDefault=false\norg.example.player/.Search"] {
            assert_eq!(search_handler(raw, "org.example.player"), NativeSearch::Handler { component:"org.example.player/.Search".into() });
        }
    }
}
