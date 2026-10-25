//! Only service-owned files are created/removed; no recursive deletion.
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
};
use tv_domain::{Failure, PortResult};

fn fail() -> Failure {
    Failure::before("service_artifact_unsafe")
}
fn directory(path: &Path) -> PortResult<()> {
    for ancestor in path.ancestors() {
        let metadata = ancestor.symlink_metadata().map_err(|_| fail())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(fail());
        }
    }
    let metadata = path.symlink_metadata().map_err(|_| fail())?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        return Err(fail());
    }
    Ok(())
}
fn sync_parent(path: &Path) -> PortResult<()> {
    File::open(path.parent().ok_or_else(fail)?)
        .and_then(|file| file.sync_all())
        .map_err(|_| Failure::uncertain("service_artifact_sync_failed"))
}
fn same_file(file: &File, path: &Path) -> PortResult<()> {
    let held = file.metadata().map_err(|_| fail())?;
    let named = path.symlink_metadata().map_err(|_| fail())?;
    if !held.is_file()
        || held.nlink() != 1
        || held.uid() != unsafe { libc::geteuid() }
        || held.mode() & 0o7777 != 0o600
        || held.dev() != named.dev()
        || held.ino() != named.ino()
        || named.file_type().is_symlink()
    {
        return Err(fail());
    }
    Ok(())
}
pub(super) fn create(path: &Path, expected: &[u8]) -> PortResult<()> {
    if expected.len() > 65_536 {
        return Err(fail());
    }
    let parent = path.parent().ok_or_else(fail)?;
    // Reject pre-existing links before create_dir_all can traverse them and
    // create a child in another tree. Recheck the resulting path afterwards.
    for ancestor in parent.ancestors() {
        match ancestor.symlink_metadata() {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(fail()),
        }
    }
    // Future explicit install only; never called by plan or the ordinary installer.
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(|_| fail())?;
    directory(parent)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| Failure::before("service_artifact_exists_or_unwritable"))?;
    file.write_all(expected)
        .and_then(|_| file.sync_all())
        .map_err(|_| Failure::uncertain("service_artifact_write_failed"))?;
    same_file(&file, path)?;
    sync_parent(path)
}
pub(super) fn verify(path: &Path, expected: &[u8]) -> PortResult<()> {
    directory(path.parent().ok_or_else(fail)?)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| fail())?;
    same_file(&file, path)?;
    let mut bytes = Vec::new();
    (&file)
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|_| fail())?;
    same_file(&file, path)?;
    if bytes != expected {
        return Err(fail());
    }
    Ok(())
}
pub(super) fn remove(path: &Path, expected: &[u8]) -> PortResult<()> {
    verify(path, expected)?;
    fs::remove_file(path).map_err(|_| Failure::uncertain("service_artifact_remove_failed"))?;
    sync_parent(path)
}
