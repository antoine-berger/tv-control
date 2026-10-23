//! Private onboarding choices and bounded event codes. Nothing stored here is an access grant.
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

pub const MAX_LOG_BYTES: usize = 65_536;
const MAX_STATE_BYTES: usize = 8192;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProgressStage {
    Configuration,
    AdbConsent,
    AdbRuntime,
    DeviceObservation,
    LocalMcp,
    TunnelConsent,
    TunnelRuntime,
    ExternalConversation,
    Complete,
}
impl From<tv_domain::pipeline::Stage> for ProgressStage {
    fn from(stage: tv_domain::pipeline::Stage) -> Self {
        use tv_domain::pipeline::Stage;
        match stage {
            Stage::Configuration => Self::Configuration,
            Stage::AdbConsent => Self::AdbConsent,
            Stage::AdbRuntime => Self::AdbRuntime,
            Stage::DeviceObservation => Self::DeviceObservation,
            Stage::LocalMcp => Self::LocalMcp,
            Stage::TunnelConsent => Self::TunnelConsent,
            Stage::TunnelRuntime => Self::TunnelRuntime,
            Stage::ExternalConversation => Self::ExternalConversation,
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProgressOutcome {
    Pending,
    Blocked,
    Rejected,
    Cancelled,
    Expired,
    Failed,
    Verified,
    Simulated,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub schema: u32,
    pub installation_id: String,
    pub configuration_path: PathBuf,
    pub configuration_revision: String,
    pub device: String,
    pub stage: ProgressStage,
    pub outcome: ProgressOutcome,
    pub tunnel_id: Option<String>,
    pub tunnel_alias: Option<String>,
}
impl Checkpoint {
    pub fn validate(&self) -> Result<(), &'static str> {
        let token = |s: &str| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        };
        if self.schema != 1
            || !tv_domain::identifier(&self.installation_id)
            || !self.configuration_path.is_absolute()
            || self.configuration_path.as_os_str().len() > 4096
            || self
                .configuration_path
                .to_string_lossy()
                .chars()
                .any(char::is_control)
            || !token(&self.configuration_revision)
            || !tv_domain::identifier(&self.device)
            || self
                .tunnel_alias
                .as_ref()
                .is_some_and(|s| !tv_domain::identifier(s))
            || self
                .tunnel_id
                .as_ref()
                .is_some_and(|s| !s.starts_with("tunnel_") || !token(s))
        {
            return Err("pipeline_checkpoint_invalid");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventCode {
    Started,
    Resumed,
    AwaitingApproval,
    ApprovalRejected,
    Checking,
    Observed,
    RuntimeStarted,
    RuntimeStopped,
    WaitingForConversation,
    Cancelled,
    Expired,
    DependencyMissing,
    DeviceUnauthorized,
    DeviceUnavailable,
    ProtocolError,
    TunnelUnavailable,
    EvidenceMismatch,
    StorageConflict,
    Failed,
    Complete,
}
/// Deliberately no text, path, identifier, packet, credential or subprocess output field.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LogEvent {
    pub stage: ProgressStage,
    pub code: EventCode,
    pub elapsed_ms: u64,
}

struct Snapshot {
    file: File,
    bytes: Vec<u8>,
}
struct AtomicFile {
    path: PathBuf,
    limit: usize,
    snapshot: Option<Snapshot>,
}
fn private_file(file: &File) -> Result<fs::Metadata, &'static str> {
    let m = file.metadata().map_err(|_| "pipeline_file_unreadable")?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.permissions().mode() & 0o777 != 0o600
    {
        return Err("pipeline_file_not_private");
    }
    Ok(m)
}
fn read(file: &mut File, limit: usize) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "pipeline_file_unreadable")?;
    if bytes.len() > limit {
        return Err("pipeline_file_too_large");
    }
    Ok(bytes)
}
fn open_private(path: &Path) -> Result<File, &'static str> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| "pipeline_file_unreadable")?;
    private_file(&file)?;
    Ok(file)
}
impl AtomicFile {
    fn open(path: PathBuf, limit: usize) -> Result<Self, &'static str> {
        let snapshot = match path.symlink_metadata() {
            Ok(_) => {
                let mut file = open_private(&path)?;
                let metadata = private_file(&file)?;
                if metadata.nlink() != 1 {
                    Self::recover_links(&path, &metadata)?;
                }
                Some(Snapshot {
                    bytes: read(&mut file, limit)?,
                    file,
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("pipeline_file_unreadable"),
        };
        Ok(Self {
            path,
            limit,
            snapshot,
        })
    }
    fn recover_links(path: &Path, metadata: &fs::Metadata) -> Result<(), &'static str> {
        let mut links = Vec::new();
        let prefix = format!(".pipeline-{}-", path.file_name().unwrap().to_string_lossy());
        for entry in fs::read_dir(path.parent().unwrap())
            .map_err(|_| "pipeline_file_unreadable")?
            .take(256)
        {
            let entry = entry.map_err(|_| "pipeline_file_unreadable")?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(numbers) = name
                .strip_prefix(&prefix)
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
                .map_err(|_| "pipeline_file_unreadable")?;
            if m.is_file() && (m.dev(), m.ino()) == (metadata.dev(), metadata.ino()) {
                links.push(entry.path());
            }
        }
        if links.len() as u64 + 1 != metadata.nlink() {
            return Err("pipeline_unrecognized_links");
        }
        for link in links {
            fs::remove_file(link).map_err(|_| "pipeline_recovery_failed")?;
        }
        Ok(())
    }
    fn bytes(&self) -> Option<&[u8]> {
        self.snapshot.as_ref().map(|s| s.bytes.as_slice())
    }
    fn check(&self) -> Result<(), &'static str> {
        if let Some(snapshot) = &self.snapshot {
            let mut current = open_private(&self.path).map_err(|_| "pipeline_storage_conflict")?;
            let expected = private_file(&snapshot.file).map_err(|_| "pipeline_storage_conflict")?;
            let actual = private_file(&current).map_err(|_| "pipeline_storage_conflict")?;
            if (expected.dev(), expected.ino()) != (actual.dev(), actual.ino())
                || actual.nlink() != 1
                || read(&mut current, self.limit).map_err(|_| "pipeline_storage_conflict")?
                    != snapshot.bytes
            {
                return Err("pipeline_storage_conflict");
            }
        } else if self.path.symlink_metadata().is_ok() {
            return Err("pipeline_storage_conflict");
        }
        Ok(())
    }
    fn write(&mut self, bytes: Vec<u8>) -> Result<(), &'static str> {
        if bytes.len() > self.limit {
            return Err("pipeline_file_too_large");
        }
        self.check()?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temp = self.path.with_file_name(format!(
            ".pipeline-{}-{}-{}.tmp",
            self.path.file_name().unwrap().to_string_lossy(),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temp)
            .map_err(|_| "pipeline_temporary_failed")?;
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.0);
            }
        }
        let cleanup = Cleanup(temp.clone());
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "pipeline_write_failed")?;
        self.check()?;
        if self.snapshot.is_some() {
            fs::rename(&temp, &self.path).map_err(|_| "pipeline_publish_failed")?;
        } else {
            fs::hard_link(&temp, &self.path).map_err(|_| "pipeline_storage_conflict")?;
            fs::remove_file(&temp).map_err(|_| "pipeline_publish_failed")?;
        }
        drop(cleanup);
        File::open(self.path.parent().unwrap())
            .and_then(|d| d.sync_all())
            .map_err(|_| "pipeline_sync_failed")?;
        self.snapshot = Some(Snapshot { file, bytes });
        Ok(())
    }
}

struct SessionLock(File);
impl Drop for SessionLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
pub struct Store {
    directory: PathBuf,
    _lock: SessionLock,
    checkpoint: AtomicFile,
    log: AtomicFile,
}
impl Store {
    pub fn open(directory: &Path) -> Result<Self, &'static str> {
        if !directory.is_absolute() {
            return Err("pipeline_directory_invalid");
        }
        let parent = directory.parent().ok_or("pipeline_directory_invalid")?;
        for path in parent.ancestors() {
            if path
                .symlink_metadata()
                .map_err(|_| "pipeline_directory_missing")?
                .file_type()
                .is_symlink()
            {
                return Err("pipeline_symlink_refused");
            }
        }
        match fs::create_dir(directory) {
            Ok(()) => fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .map_err(|_| "pipeline_directory_not_private")?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("pipeline_directory_missing"),
        }
        let m = directory
            .symlink_metadata()
            .map_err(|_| "pipeline_directory_missing")?;
        if !m.is_dir()
            || m.uid() != unsafe { libc::geteuid() }
            || m.permissions().mode() & 0o777 != 0o700
        {
            return Err("pipeline_directory_not_private");
        }
        let directory = directory
            .canonicalize()
            .map_err(|_| "pipeline_directory_missing")?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(directory.join("session.lock"))
            .map_err(|_| "pipeline_lock_failed")?;
        if private_file(&lock)?.nlink() != 1 {
            return Err("pipeline_unrecognized_links");
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(match std::io::Error::last_os_error().raw_os_error() {
                Some(n) if n == libc::EWOULDBLOCK || n == libc::EAGAIN => "pipeline_busy",
                Some(libc::EINTR) => "pipeline_lock_interrupted",
                _ => "pipeline_lock_failed",
            });
        }
        let lock = SessionLock(lock);
        let checkpoint = AtomicFile::open(directory.join("progress.json"), MAX_STATE_BYTES)?;
        let log = AtomicFile::open(directory.join("events.jsonl"), MAX_LOG_BYTES)?;
        Ok(Self {
            directory,
            _lock: lock,
            checkpoint,
            log,
        })
    }
    pub fn load(&self) -> Result<Option<Checkpoint>, &'static str> {
        self.checkpoint.check()?;
        let Some(bytes) = self.checkpoint.bytes() else {
            return Ok(None);
        };
        let checkpoint: Checkpoint =
            serde_json::from_slice(bytes).map_err(|_| "pipeline_checkpoint_corrupt")?;
        checkpoint.validate()?;
        Ok(Some(checkpoint))
    }
    pub fn save(&mut self, checkpoint: &Checkpoint) -> Result<(), &'static str> {
        // Refuse silently replacing a corrupt checkpoint even if save is called before load.
        self.load()?;
        checkpoint.validate()?;
        self.checkpoint
            .write(serde_json::to_vec(checkpoint).map_err(|_| "pipeline_encode_failed")?)
    }
    /// Call only after explicit consent. Preserve unreadable schema/content in a
    /// separate private file; do not repair permissions or unsafe file types.
    pub fn quarantine_checkpoint(&mut self) -> Result<PathBuf, &'static str> {
        self.checkpoint.check()?;
        if self.checkpoint.snapshot.is_none() {
            return Err("pipeline_checkpoint_missing");
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let backup = self.directory.join(format!(
            "progress.corrupt-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let reservation = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&backup)
            .map_err(|_| "pipeline_backup_failed")?;
        self.checkpoint.check()?;
        fs::rename(&self.checkpoint.path, &backup).map_err(|_| "pipeline_backup_failed")?;
        drop(reservation);
        self.checkpoint.snapshot = None;
        File::open(&self.directory)
            .and_then(|d| d.sync_all())
            .map_err(|_| "pipeline_sync_failed")?;
        Ok(backup)
    }
    pub fn event(&mut self, event: LogEvent) -> Result<(), &'static str> {
        let mut bytes = self.log.bytes().unwrap_or_default().to_vec();
        for line in bytes.split(|b| *b == b'\n').filter(|s| !s.is_empty()) {
            serde_json::from_slice::<LogEvent>(line).map_err(|_| "pipeline_log_corrupt")?;
        }
        let mut next = serde_json::to_vec(&event).map_err(|_| "pipeline_encode_failed")?;
        next.push(b'\n');
        while bytes.len() + next.len() > MAX_LOG_BYTES {
            let index = bytes
                .iter()
                .position(|b| *b == b'\n')
                .ok_or("pipeline_log_corrupt")?;
            bytes.drain(..=index);
        }
        bytes.extend(next);
        self.log.write(bytes)
    }
    pub fn log_path(&self) -> PathBuf {
        self.directory.join("events.jsonl")
    }
}
