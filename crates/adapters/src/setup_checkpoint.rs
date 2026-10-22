//! Private local wizard progress. No credentials, authorization claims or network operations.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub schema: u32,
    pub writer_version: String,
    pub output: PathBuf,
    pub interface: Option<String>,
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub manual_endpoint: Option<String>,
    pub manual_kind: Option<String>,
    pub adb: Option<PathBuf>,
    pub curl: Option<PathBuf>,
    pub server_port: Option<u16>,
    pub configuration_saved: bool,
}
impl Checkpoint {
    pub fn validate(&self) -> Result<(), &'static str> {
        let clean = |s: &str| s.len() <= 4096 && !s.chars().any(char::is_control);
        if self.schema != 1
            || !self.output.is_absolute()
            || !clean(&self.output.to_string_lossy())
            || self.writer_version.is_empty()
            || self.writer_version.len() > 64
            || !self
                .writer_version
                .bytes()
                .all(|c| c.is_ascii_digit() || c == b'.')
            || self
                .interface
                .as_ref()
                .is_some_and(|s| s.len() > 128 || !clean(s))
            || self
                .device
                .as_ref()
                .is_some_and(|s| !tv_domain::identifier(s))
            || self
                .manual_endpoint
                .as_ref()
                .is_some_and(|s| s.parse::<std::net::SocketAddrV4>().is_err())
            || self
                .manual_kind
                .as_ref()
                .is_some_and(|s| s != "legacy" && s != "tls")
            || self
                .adb
                .iter()
                .chain(self.curl.iter())
                .any(|p| !p.is_absolute() || !clean(&p.to_string_lossy()))
            || self
                .display_name
                .as_ref()
                .is_some_and(|s| !crate::config::valid_display_name(s))
            || self.server_port == Some(0)
        {
            return Err("checkpoint_invalid");
        }
        Ok(())
    }
}
pub struct Store {
    path: PathBuf,
    _lock: File,
    expected: Option<(u64, u64)>,
    expected_bytes: Option<Vec<u8>>,
    source: Option<File>,
}
impl Drop for Store {
    fn drop(&mut self) {
        // Release the session explicitly. A concurrent fork can briefly retain
        // the same open-file description even with close-on-exec enabled.
        unsafe {
            libc::flock(self._lock.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
fn private_file(file: &File) -> Result<fs::Metadata, &'static str> {
    let m = file.metadata().map_err(|_| "checkpoint_unreadable")?;
    if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.permissions().mode() & 0o077 != 0
    {
        return Err("checkpoint_not_private");
    }
    if m.nlink() != 1 {
        return Err("checkpoint_multiple_links");
    }
    Ok(m)
}
fn lock_error(errno: Option<i32>) -> &'static str {
    if errno.is_some_and(|n| n == libc::EWOULDBLOCK || n == libc::EAGAIN) {
        "checkpoint_busy"
    } else if errno == Some(libc::EINTR) {
        "checkpoint_lock_interrupted"
    } else {
        "checkpoint_lock_failed"
    }
}
impl Store {
    /// Finish only the exact duplicate links left by our initial atomic publication.
    /// Unrecognized hard links remain errors; the checkpoint data is never deleted.
    pub fn recover_publication(&mut self) -> Result<(), &'static str> {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)
            .map_err(|_| "checkpoint_unreadable")?;
        let metadata = file.metadata().map_err(|_| "checkpoint_unreadable")?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
            || metadata.nlink() < 2
        {
            return Err("checkpoint_not_private");
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(8193)
            .read_to_end(&mut bytes)
            .map_err(|_| "checkpoint_unreadable")?;
        let state: Checkpoint =
            serde_json::from_slice(&bytes).map_err(|_| "checkpoint_corrupt_use_new_path")?;
        state.validate()?;
        let mut links = Vec::new();
        for entry in fs::read_dir(self.path.parent().unwrap())
            .map_err(|_| "checkpoint_unreadable")?
            .take(256)
        {
            let entry = entry.map_err(|_| "checkpoint_unreadable")?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(numbers) = name
                .strip_prefix(".tv-setup-")
                .and_then(|s| s.strip_suffix(".tmp"))
            else {
                continue;
            };
            let parts: Vec<_> = numbers.split('-').collect();
            if parts.len() != 2
                || parts
                    .iter()
                    .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
            {
                continue;
            }
            let m = entry
                .path()
                .symlink_metadata()
                .map_err(|_| "checkpoint_unreadable")?;
            if m.is_file() && (m.dev(), m.ino()) == (metadata.dev(), metadata.ino()) {
                links.push(entry.path());
            }
        }
        if links.len() as u64 + 1 != metadata.nlink() {
            return Err("checkpoint_unrecognized_links");
        }
        for link in links {
            fs::remove_file(link).map_err(|_| "checkpoint_recovery_failed")?;
        }
        self.expected = None;
        self.expected_bytes = None;
        self.source = None;
        Ok(())
    }
    /// Keep an unreadable private checkpoint without exposing or discarding its contents.
    /// The caller must obtain explicit consent; unsafe file types/permissions stay errors.
    pub fn quarantine(&mut self) -> Result<(), &'static str> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)
            .map_err(|_| "checkpoint_unreadable")?;
        let metadata = private_file(&file)?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let backup = self.path.with_file_name(format!(
            "{}.corrupt-{}-{}",
            self.path.file_name().unwrap().to_string_lossy(),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _reservation = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&backup)
            .map_err(|_| "checkpoint_backup_failed")?;
        let current = self
            .path
            .symlink_metadata()
            .map_err(|_| "checkpoint_conflict")?;
        if !current.is_file() || (current.dev(), current.ino()) != (metadata.dev(), metadata.ino())
        {
            return Err("checkpoint_conflict");
        }
        // Replace only our reserved empty backup, in one atomic rename. A crash
        // cannot leave the source with an additional hard link.
        fs::rename(&self.path, &backup).map_err(|_| "checkpoint_backup_failed")?;
        self.expected = None;
        self.expected_bytes = None;
        self.source = None;
        Ok(())
    }
    pub fn open(path: &Path) -> Result<Self, &'static str> {
        let parent = path.parent().ok_or("checkpoint_directory_missing")?;
        for part in parent.ancestors() {
            if part
                .symlink_metadata()
                .map_err(|_| "checkpoint_directory_missing")?
                .file_type()
                .is_symlink()
            {
                return Err("checkpoint_symlink_refused");
            }
        }
        let parent = parent
            .canonicalize()
            .map_err(|_| "checkpoint_directory_missing")?;
        let m = parent
            .metadata()
            .map_err(|_| "checkpoint_directory_missing")?;
        if m.uid() != unsafe { libc::geteuid() } || m.permissions().mode() & 0o022 != 0 {
            return Err("checkpoint_directory_not_private");
        }
        let path = parent.join(path.file_name().ok_or("checkpoint_invalid_path")?);
        let lock_path = path.with_file_name(format!(
            "{}.lock",
            path.file_name().unwrap().to_string_lossy()
        ));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(lock_path)
            .map_err(|_| "checkpoint_lock_failed")?;
        private_file(&lock)?;
        // Advisory lock is released by the OS after a crash; no stale PID deletion.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(lock_error(std::io::Error::last_os_error().raw_os_error()));
        }
        Ok(Self {
            path,
            _lock: lock,
            expected: None,
            expected_bytes: None,
            source: None,
        })
    }
    pub fn load(&mut self) -> Result<Option<Checkpoint>, &'static str> {
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("checkpoint_unreadable"),
        };
        let metadata = private_file(&file)?;
        let mut bytes = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(8193)
            .read_to_end(&mut bytes)
            .map_err(|_| "checkpoint_unreadable")?;
        if bytes.len() > 8192 {
            return Err("checkpoint_too_large");
        }
        let checkpoint: Checkpoint =
            serde_json::from_slice(&bytes).map_err(|_| "checkpoint_corrupt_use_new_path")?;
        checkpoint.validate()?;
        self.expected = Some((metadata.dev(), metadata.ino()));
        self.expected_bytes = Some(bytes);
        self.source = Some(file);
        Ok(Some(checkpoint))
    }
    pub fn save(&mut self, checkpoint: &Checkpoint) -> Result<(), &'static str> {
        checkpoint.validate()?;
        let bytes =
            serde_json::to_vec_pretty(checkpoint).map_err(|_| "checkpoint_encode_failed")?;
        if bytes.len() > 8192 {
            return Err("checkpoint_too_large");
        }
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let temporary = self.path.with_file_name(format!(
            ".tv-setup-{}-{}.tmp",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)
            .map_err(|_| "checkpoint_temporary_failed")?;
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.0);
            }
        }
        let _cleanup = Cleanup(temporary.clone());
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "checkpoint_write_failed")?;
        if let Some(expected) = self.expected {
            let actual = self
                .path
                .symlink_metadata()
                .map_err(|_| "checkpoint_conflict")?;
            if !actual.is_file() || (actual.dev(), actual.ino()) != expected {
                return Err("checkpoint_conflict");
            }
            let mut current = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&self.path)
                .map_err(|_| "checkpoint_conflict")?;
            let m = private_file(&current).map_err(|_| "checkpoint_conflict")?;
            let mut current_bytes = Vec::new();
            Read::by_ref(&mut current)
                .take(8193)
                .read_to_end(&mut current_bytes)
                .map_err(|_| "checkpoint_conflict")?;
            if (m.dev(), m.ino()) != expected
                || self.expected_bytes.as_deref() != Some(current_bytes.as_slice())
            {
                return Err("checkpoint_conflict");
            }
            fs::rename(&temporary, &self.path).map_err(|_| "checkpoint_publish_failed")?;
        } else {
            fs::hard_link(&temporary, &self.path).map_err(|_| "checkpoint_conflict")?;
        }
        let m = self.path.metadata().map_err(|_| "checkpoint_unreadable")?;
        self.expected = Some((m.dev(), m.ino()));
        self.expected_bytes = Some(bytes);
        self.source = Some(file);
        if let Some(parent) = self.path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn lock_contention_is_distinct_from_interruption_and_other_errors() {
        assert_eq!(
            super::lock_error(Some(libc::EWOULDBLOCK)),
            "checkpoint_busy"
        );
        assert_eq!(
            super::lock_error(Some(libc::EINTR)),
            "checkpoint_lock_interrupted"
        );
        assert_eq!(
            super::lock_error(Some(libc::EINVAL)),
            "checkpoint_lock_failed"
        );
        assert_eq!(super::lock_error(None), "checkpoint_lock_failed");
    }
}
