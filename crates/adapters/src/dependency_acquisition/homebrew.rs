use crate::process::{Invocation, Process};
use std::{collections::BTreeMap, path::Path};
use tv_application::Context;

/// Official supported macOS path. Caller explains that Homebrew manages its own
/// prefix and current formula version; this is not a pinned private Linux install.
pub fn install_homebrew(
    brew: &Path,
    approved: bool,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> Result<(), &'static str> {
    if !approved {
        return Err("tunnel_dependency_approval_required");
    }
    if !brew.is_absolute() {
        return Err("tunnel_dependency_brew_path_invalid");
    }
    install_homebrew_package(brew, approved, "openai/tools/tunnel-client", process, ctx)
}
pub fn install_adb_homebrew(
    brew: &Path,
    approved: bool,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> Result<(), &'static str> {
    install_homebrew_package(brew, approved, "android-platform-tools", process, ctx)
}
fn install_homebrew_package(
    brew: &Path,
    approved: bool,
    package: &str,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> Result<(), &'static str> {
    if !approved {
        return Err("tunnel_dependency_approval_required");
    }
    if !brew.is_absolute() {
        return Err("tunnel_dependency_brew_path_invalid");
    }
    let mut env = BTreeMap::new();
    for key in ["HOME", "PATH", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) {
            env.insert(key.into(), value);
        }
    }
    env.insert("HOMEBREW_NO_AUTO_UPDATE".into(), "1".into());
    env.insert("HOMEBREW_NO_ANALYTICS".into(), "1".into());
    env.insert("HOMEBREW_NO_ENV_HINTS".into(), "1".into());
    let result = process
        .run(
            Invocation {
                program: brew.to_owned(),
                args: vec!["install".into(), package.into()],
                env,
                stdin: vec![],
                mutating: true,
            },
            ctx,
        )
        .map_err(|e| e.code)?;
    if !result.success {
        return Err("tunnel_dependency_homebrew_failed");
    }
    Ok(())
}
