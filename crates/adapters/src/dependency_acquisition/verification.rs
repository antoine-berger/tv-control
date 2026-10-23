use super::{
    digest,
    filesystem::{private_directory, private_file},
    Release,
};
use crate::process::Process;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use tv_application::Context;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub(super) owner: String,
    pub(super) archive_sha256: String,
    pub(super) files: BTreeMap<String, String>,
}
/// Read-only reuse. A foreign, modified, partial, or wrong-platform installation fails closed.
pub fn installed(
    state: &Path,
    release: Release,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> Result<Option<PathBuf>, &'static str> {
    private_directory(state)?;
    let directory = release.directory(state);
    match directory.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("tunnel_dependency_directory_unreadable"),
        Ok(_) => {}
    }
    private_directory(&directory)?;
    let mut bytes = Vec::new();
    private_file(&directory.join("receipt.json"), false)?
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| "tunnel_dependency_receipt_unreadable")?;
    if bytes.len() > 8192 {
        return Err("tunnel_dependency_receipt_invalid");
    }
    let receipt: Receipt =
        serde_json::from_slice(&bytes).map_err(|_| "tunnel_dependency_receipt_invalid")?;
    let members = release.payload();
    if receipt.owner != release.owner()
        || receipt.archive_sha256 != release.sha256
        || receipt.files.len() != members.len()
    {
        return Err("tunnel_dependency_receipt_mismatch");
    }
    if fs::read_dir(&directory)
        .map_err(|_| "tunnel_dependency_directory_unreadable")?
        .count()
        != members.len() + 1
    {
        return Err("tunnel_dependency_unexpected_file");
    }
    for (_, member, executable) in members {
        let file = directory.join(&member);
        private_file(&file, executable)?;
        if receipt.files.get(&member) != Some(&digest(process, &file, ctx)?) {
            return Err("tunnel_dependency_file_modified");
        }
    }
    Ok(Some(directory.join(release.binary())))
}
/// Verify only our generated private ADB namespace. Other executable paths remain
/// explicit trusted configuration. Call before resolving symlinks or executing ADB.
pub fn verify_adb_path(
    binary: &Path,
    configuration_parent: &Path,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> Result<(), &'static str> {
    let parent = configuration_parent
        .canonicalize()
        .map_err(|_| "tunnel_dependency_directory_missing")?;
    let state = parent.join("connection-dependencies");
    // Resolve aliases above the private namespace, never its executable leaf.
    // Canonicalizing the whole binary would hide a replaced private symlink by
    // turning it into an apparently external, caller-configured executable.
    let normalized = if binary.starts_with(&state) {
        binary.to_owned()
    } else if let Some(namespace) = binary.ancestors().find(|path| {
        path.file_name()
            .is_some_and(|name| name == "connection-dependencies")
            && path.parent().and_then(|p| p.canonicalize().ok()).as_ref() == Some(&parent)
    }) {
        state.join(
            binary
                .strip_prefix(namespace)
                .map_err(|_| "adb_private_installation_path_invalid")?,
        )
    } else {
        // Also recognize an explicit alias to the managed version directory.
        // Preserve the final component so installed() can reject a leaf symlink.
        let Some(candidate) = binary
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .and_then(|p| binary.file_name().map(|name| p.join(name)))
        else {
            return Ok(());
        };
        if !candidate.starts_with(&state) {
            return Ok(());
        }
        candidate
    };
    let binary = normalized.as_path();
    if std::env::consts::OS != "linux" {
        return Err("adb_private_installation_platform_mismatch");
    }
    let release = Release::adb_linux(std::env::consts::ARCH)?;
    if binary != release.binary_path(&state) {
        return Err("adb_private_installation_path_invalid");
    }
    if installed(&state, release, process, ctx)?.as_deref() != Some(binary) {
        return Err("adb_private_installation_missing");
    }
    Ok(())
}
