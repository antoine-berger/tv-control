use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

/// Create only a private dependency root below a caller-owned, non-writable parent.
pub fn prepare_directory(parent: &Path, approved: bool) -> Result<PathBuf, &'static str> {
    if !approved {
        return Err("tunnel_dependency_approval_required");
    }
    if !parent.is_absolute() {
        return Err("tunnel_dependency_path_invalid");
    }
    for path in parent.ancestors() {
        if path
            .symlink_metadata()
            .map_err(|_| "tunnel_dependency_directory_missing")?
            .file_type()
            .is_symlink()
        {
            return Err("tunnel_dependency_symlink_refused");
        }
    }
    let m = parent
        .metadata()
        .map_err(|_| "tunnel_dependency_directory_missing")?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o022 != 0 {
        return Err("tunnel_dependency_directory_not_private");
    }
    let path = parent.join("connection-dependencies");
    match fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err("tunnel_dependency_stage_failed"),
    }
    private_directory(&path)?;
    Ok(path)
}
pub(super) fn private_directory(path: &Path) -> Result<(), &'static str> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("tunnel_dependency_path_invalid");
    }
    for ancestor in path.ancestors() {
        if ancestor
            .symlink_metadata()
            .map_err(|_| "tunnel_dependency_directory_missing")?
            .file_type()
            .is_symlink()
        {
            return Err("tunnel_dependency_symlink_refused");
        }
    }
    let m = path
        .symlink_metadata()
        .map_err(|_| "tunnel_dependency_directory_missing")?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o777 != 0o700 {
        return Err("tunnel_dependency_directory_not_private");
    }
    Ok(())
}
pub(super) fn private_file(path: &Path, executable: bool) -> Result<File, &'static str> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "tunnel_dependency_file_unreadable")?;
    let m = f
        .metadata()
        .map_err(|_| "tunnel_dependency_file_unreadable")?;
    let mode = if executable { 0o700 } else { 0o600 };
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != mode
    {
        return Err("tunnel_dependency_file_not_private");
    }
    Ok(f)
}
pub(super) fn write_new(path: &Path, bytes: &[u8], executable: bool) -> Result<(), &'static str> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| "tunnel_dependency_file_exists")?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "tunnel_dependency_write_failed")?;
    if executable {
        file.set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|_| "tunnel_dependency_write_failed")?;
    }
    Ok(())
}
