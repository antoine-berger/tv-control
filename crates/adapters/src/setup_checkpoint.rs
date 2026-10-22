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
}
fn private_file(file: &File) -> Result<fs::Metadata, &'static str> {
    let m = file.metadata().map_err(|_| "checkpoint_unreadable")?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.permissions().mode() & 0o077 != 0
        || m.nlink() != 1
    {
        return Err("checkpoint_not_private");
    }
    Ok(m)
}
impl Store {
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
            return Err("checkpoint_busy");
        }
        Ok(Self {
            path,
            _lock: lock,
            expected: None,
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
            fs::rename(&temporary, &self.path).map_err(|_| "checkpoint_publish_failed")?;
        } else {
            fs::hard_link(&temporary, &self.path).map_err(|_| "checkpoint_conflict")?;
        }
        let m = self.path.metadata().map_err(|_| "checkpoint_unreadable")?;
        self.expected = Some((m.dev(), m.ino()));
        if let Some(parent) = self.path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    }
}
