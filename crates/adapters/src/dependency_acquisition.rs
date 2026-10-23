//! Dependency acquisition only: never authenticates, starts a tunnel, or runs the payload.
//! Linux archives are pinned to release digests. macOS uses the upstream-supported
//! Homebrew formula; it deliberately has a separate, explicit consent boundary.
mod filesystem;
mod homebrew;
mod release;
mod verification;

pub use filesystem::prepare_directory;
pub use homebrew::{install_adb_homebrew, install_homebrew};
pub use release::{Release, VERSION};
pub use verification::{installed, verify_adb_path};

use crate::process::{Invocation, Process};
use filesystem::{private_directory, private_file, write_new};
use release::executable_header;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
use tv_application::Context;
use verification::Receipt;

const MAX_ARCHIVE: u64 = 32 * 1024 * 1024;
const MAX_MEMBER: usize = 64 * 1024 * 1024;

fn system_program(name: &str) -> Result<PathBuf, &'static str> {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|p| Path::new(p).join(name))
        .find(|p| p.is_file())
        .ok_or("tunnel_dependency_system_tool_missing")
}
fn invoke(
    process: &dyn Process,
    program: PathBuf,
    args: Vec<OsString>,
    ctx: &Context<'_>,
) -> Result<Vec<u8>, &'static str> {
    let env = BTreeMap::from([
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("LC_ALL".into(), "C".into()),
    ]);
    let output = process
        .run(
            Invocation {
                program,
                args,
                env,
                stdin: vec![],
                mutating: false,
            },
            ctx,
        )
        .map_err(|e| e.code)?;
    if !output.success {
        return Err("tunnel_dependency_tool_failed");
    }
    Ok(output.stdout)
}
fn digest(process: &dyn Process, path: &Path, ctx: &Context<'_>) -> Result<String, &'static str> {
    let (program, args) = match system_program("sha256sum") {
        Ok(program) => (program, vec![path.as_os_str().into()]),
        Err(_) => (
            system_program("shasum")?,
            vec!["-a".into(), "256".into(), path.as_os_str().into()],
        ),
    };
    let output = invoke(process, program, args, ctx)?;
    let value = std::str::from_utf8(&output)
        .map_err(|_| "tunnel_dependency_digest_invalid")?
        .split_whitespace()
        .next()
        .ok_or("tunnel_dependency_digest_invalid")?;
    if value.len() != 64 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("tunnel_dependency_digest_invalid");
    }
    Ok(value.to_ascii_lowercase())
}
struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
/// Caller must obtain approval first. No credential, runtime, or account operations occur here.
pub fn install_linux(
    state: &Path,
    release: Release,
    approved: bool,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> Result<PathBuf, &'static str> {
    if !approved {
        return Err("tunnel_dependency_approval_required");
    }
    ctx.check().map_err(|e| e.code)?;
    private_directory(state)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(state.join("dependency.lock"))
        .map_err(|_| "tunnel_dependency_lock_failed")?;
    private_file(&state.join("dependency.lock"), false)?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("tunnel_dependency_locked");
    }
    if let Some(binary) = installed(state, release, process, ctx)? {
        return Ok(binary);
    }
    let stage = state.join(format!(
        ".dependency-{}",
        crate::connection_proof::random_token()?
    ));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&stage)
        .map_err(|_| "tunnel_dependency_stage_failed")?;
    let stage = Stage(stage);
    let archive = stage.0.join("archive.zip");
    write_new(&archive, &[], false)?;
    invoke(
        process,
        system_program("curl")?,
        [
            "-q",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "--connect-timeout",
            "10",
            "--max-time",
            "120",
            "--max-filesize",
            "33554432",
            "--output",
        ]
        .into_iter()
        .map(OsString::from)
        .chain([archive.as_os_str().into(), release.url().into()])
        .collect(),
        ctx,
    )?;
    if private_file(&archive, false)?
        .metadata()
        .map_err(|_| "tunnel_dependency_file_unreadable")?
        .len()
        > MAX_ARCHIVE
    {
        return Err("tunnel_dependency_archive_too_large");
    }
    if digest(process, &archive, ctx)? != release.sha256 {
        return Err("tunnel_dependency_checksum_mismatch");
    }
    let list = invoke(
        process,
        system_program("unzip")?,
        vec!["-Z1".into(), archive.as_os_str().into()],
        ctx,
    )?;
    let text = std::str::from_utf8(&list).map_err(|_| "tunnel_dependency_archive_invalid")?;
    let mut actual: Vec<_> = text.lines().collect();
    actual.sort_unstable();
    let mut expected = release.members();
    expected.sort_unstable();
    if actual != expected {
        return Err("tunnel_dependency_archive_members_invalid");
    }
    let payload = stage.0.join("payload");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&payload)
        .map_err(|_| "tunnel_dependency_stage_failed")?;
    let mut files = BTreeMap::new();
    for (source, member, executable) in release.payload() {
        ctx.check().map_err(|e| e.code)?;
        // Streaming a single exact member to memory never follows archive links or paths.
        let bytes = invoke(
            process,
            system_program("unzip")?,
            vec!["-p".into(), archive.as_os_str().into(), source.into()],
            ctx,
        )?;
        if bytes.len() > MAX_MEMBER {
            return Err("tunnel_dependency_member_too_large");
        }
        if executable {
            executable_header(&bytes, release)?;
        }
        let file = payload.join(&member);
        write_new(&file, &bytes, executable)?;
        files.insert(member, digest(process, &file, ctx)?);
    }
    let receipt = Receipt {
        owner: release.owner().into(),
        archive_sha256: release.sha256.into(),
        files,
    };
    write_new(
        &payload.join("receipt.json"),
        &serde_json::to_vec(&receipt).map_err(|_| "tunnel_dependency_receipt_invalid")?,
        false,
    )?;
    ctx.check().map_err(|e| e.code)?;
    let destination = release.directory(state);
    if destination.symlink_metadata().is_ok() {
        return Err("tunnel_dependency_destination_exists");
    }
    fs::rename(&payload, &destination).map_err(|_| "tunnel_dependency_publish_failed")?;
    installed(state, release, process, ctx)?.ok_or("tunnel_dependency_publish_failed")
}

#[cfg(test)]
mod tests;
