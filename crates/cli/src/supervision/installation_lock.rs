//! Same atomic mkdir lock as install.sh; no service admission during payload replacement.
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
};
use tv_domain::{Failure, PortResult};

pub(super) struct InstallationLock {
    path: PathBuf,
    device: u64,
    inode: u64,
    released: bool,
}

impl InstallationLock {
    pub(super) fn acquire(launcher: &Path) -> PortResult<Self> {
        let prefix = launcher
            .parent()
            .ok_or(Failure::before("installation_path_invalid"))?;
        let parent = prefix
            .parent()
            .ok_or(Failure::before("installation_path_invalid"))?;
        let name = prefix
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(Failure::before("installation_path_invalid"))?;
        for ancestor in parent.ancestors() {
            let meta = ancestor
                .symlink_metadata()
                .map_err(|_| Failure::before("installation_parent_unsafe"))?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(Failure::before("installation_parent_unsafe"));
            }
        }
        let meta = parent
            .symlink_metadata()
            .map_err(|_| Failure::before("installation_parent_unsafe"))?;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
            return Err(Failure::before("installation_parent_unsafe"));
        }
        let path = parent.join(format!(".{name}.install-lock"));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|_| Failure::before("installation_locked"))?;
        let meta = path
            .symlink_metadata()
            .map_err(|_| Failure::uncertain("installation_lock_identity_unverified"))?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o777 != 0o700
        {
            return Err(Failure::uncertain("installation_lock_identity_unverified"));
        }
        Ok(Self {
            path,
            device: meta.dev(),
            inode: meta.ino(),
            released: false,
        })
    }

    fn remove(&mut self) -> PortResult<()> {
        let meta = self
            .path
            .symlink_metadata()
            .map_err(|_| Failure::uncertain("installation_lock_cleanup_unverified"))?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.dev() != self.device
            || meta.ino() != self.inode
        {
            return Err(Failure::uncertain("installation_lock_cleanup_unverified"));
        }
        // remove_dir never removes foreign contents or recursively deletes.
        fs::remove_dir(&self.path)
            .map_err(|_| Failure::uncertain("installation_lock_cleanup_unverified"))?;
        self.released = true;
        Ok(())
    }

    pub(super) fn release(mut self) -> PortResult<()> {
        let result = self.remove();
        // A failed explicit cleanup is reported, never replayed from Drop.
        self.released = true;
        result
    }
}
impl Drop for InstallationLock {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.remove();
        }
    }
}
