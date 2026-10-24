//! Crash-safe operation deduplication shared by controller processes.
//!
//! A transaction lock serializes publication, while a per-operation lease is held
//! until completion. An abandoned reservation is an unknown outcome, never a
//! permission to replay. Unknown outcomes are retained until explicit repair.
//! Known terminal outcomes may expire after the configured retention window.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, FileExt, MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
use tv_application::generic::{Journal, Reservation};
use tv_domain::{generic::*, Failure, Observed, Playback, PlaybackEvidence, PortResult, Status};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TEXT: usize = 4096;

pub struct OperationJournal {
    directory: File,
    capacity: usize,
    retention_ms: u64,
    leases: Mutex<BTreeMap<String, File>>,
}

struct Transaction(File);
impl Drop for Transaction {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Disk {
    schema: u32,
    entries: BTreeMap<String, Entry>,
}
impl Default for Disk {
    fn default() -> Self {
        Self {
            schema: 1,
            entries: BTreeMap::new(),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    device: String,
    intent: IntentWire,
    resource: String,
    cancelled: bool,
    completed_at: Option<u64>,
    reply: Option<ReplyWire>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum IntentWire {
    Legacy { action: String },
    OpenApp { app: String },
    OpenUrl { app: String, url: String },
    Wake,
    Mute,
    SetVolume { index: u16 },
    Pause,
    Resume,
    Media { session: String, command: String },
    Navigate { keys: Vec<String> },
    LongSelect,
    Search { app: String, query: String },
    Activate { reference: String },
    TypeText { reference: String, text: String },
}
impl From<&Intent> for IntentWire {
    fn from(value: &Intent) -> Self {
        match value {
            Intent::Legacy { action } => Self::Legacy {
                action: action.clone(),
            },
            Intent::OpenApp { app } => Self::OpenApp { app: app.clone() },
            Intent::OpenUrl { app, url } => Self::OpenUrl {
                app: app.clone(),
                url: url.clone(),
            },
            Intent::Wake => Self::Wake,
            Intent::Mute => Self::Mute,
            Intent::SetVolume { index } => Self::SetVolume { index: *index },
            Intent::Pause => Self::Pause,
            Intent::Resume => Self::Resume,
            Intent::Media { session, command } => Self::Media {
                session: session.clone(),
                command: command.name().into(),
            },
            Intent::LongSelect => Self::LongSelect,
            Intent::Navigate { keys } => Self::Navigate {
                keys: keys
                    .iter()
                    .map(|key| {
                        match key {
                            Key::Up => "up",
                            Key::Down => "down",
                            Key::Left => "left",
                            Key::Right => "right",
                            Key::Select => "select",
                            Key::Back => "back",
                            Key::Home => "home",
                            Key::Menu => "menu",
                            Key::Settings => "settings",
                        }
                        .to_string()
                    })
                    .collect(),
            },
            Intent::Search { app, query } => Self::Search {
                app: app.clone(),
                query: query.clone(),
            },
            Intent::TypeText { reference, text } => Self::TypeText {
                reference: reference.clone(),
                text: text.clone(),
            },
            Intent::Activate { reference } => Self::Activate {
                reference: reference.clone(),
            },
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyWire {
    request_id: String,
    status: String,
    code: String,
    observed: Option<StateWire>,
    choices: Vec<AppWire>,
    cached: bool,
    local_elapsed_ms: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateWire {
    connected: bool,
    active_app: Option<String>,
    playback: String,
    playback_evidence: String,
    media_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    media_source_id: Option<String>,
    position_ticks: Option<u64>,
    #[serde(default)]
    awake: Option<bool>,
    volume_index: Option<u16>,
    #[serde(default)]
    volume_max_index: Option<u16>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppWire {
    id: String,
    component: String,
    label: Option<String>,
    search_component: Option<String>,
}

fn status_text(status: Status) -> &'static str {
    match status {
        Status::Observed => "observed",
        Status::Verified => "verified",
        Status::Invalid => "invalid",
        Status::Busy => "busy",
        Status::Unsupported => "unsupported",
        Status::Expired => "expired",
        Status::Cancelled => "cancelled",
        Status::Failed => "failed",
        Status::OutcomeUnknown => "outcome_unknown",
    }
}
impl From<&Reply> for ReplyWire {
    fn from(r: &Reply) -> Self {
        Self {
            request_id: r.request_id.clone(),
            status: status_text(r.status).into(),
            code: r.code.clone(),
            observed: r.observed.as_ref().map(|o| StateWire {
                connected: o.connected,
                active_app: o.active_app.clone(),
                playback: match o.playback {
                    Playback::Unknown => "unknown",
                    Playback::Stopped => "stopped",
                    Playback::Paused => "paused",
                    Playback::Playing => "playing",
                    Playback::Buffering => "buffering",
                }
                .into(),
                playback_evidence: match o.playback_evidence {
                    PlaybackEvidence::NativeState => "native",
                    PlaybackEvidence::PositionProgressRequired => "position",
                }
                .into(),
                media_id: o.media_id.clone(),
                media_source_id: o.media_source_id.clone(),
                position_ticks: o.position_ticks,
                awake: o.awake,
                volume_index: o.volume_index,
                volume_max_index: o.volume_max_index,
            }),
            choices: r
                .choices
                .iter()
                .map(|a| AppWire {
                    id: a.id.clone(),
                    component: a.component.clone(),
                    label: a.label.clone(),
                    search_component: a.search_component.clone(),
                })
                .collect(),
            cached: r.cached,
            local_elapsed_ms: r.local_elapsed_ms,
        }
    }
}
impl ReplyWire {
    fn decode(&self) -> Result<Reply, &'static str> {
        let bad = "operation_journal_corrupt";
        let status = match self.status.as_str() {
            "observed" => Status::Observed,
            "verified" => Status::Verified,
            "invalid" => Status::Invalid,
            "busy" => Status::Busy,
            "unsupported" => Status::Unsupported,
            "expired" => Status::Expired,
            "cancelled" => Status::Cancelled,
            "failed" => Status::Failed,
            "outcome_unknown" => Status::OutcomeUnknown,
            _ => return Err(bad),
        };
        if !tv_domain::identifier(&self.request_id)
            || self.code.len() > 256
            || self.choices.len() > 256
        {
            return Err(bad);
        }
        let observed = self
            .observed
            .as_ref()
            .map(|s| -> Result<Observed, &'static str> {
                if s.active_app.as_ref().is_some_and(|v| v.len() > MAX_TEXT)
                    || s.media_id.as_ref().is_some_and(|v| v.len() > MAX_TEXT)
                    || s.media_source_id
                        .as_ref()
                        .is_some_and(|v| v.len() > MAX_TEXT)
                {
                    return Err(bad);
                }
                Ok(Observed {
                    connected: s.connected,
                    active_app: s.active_app.clone(),
                    playback: match s.playback.as_str() {
                        "unknown" => Playback::Unknown,
                        "stopped" => Playback::Stopped,
                        "paused" => Playback::Paused,
                        "playing" => Playback::Playing,
                        "buffering" => Playback::Buffering,
                        _ => return Err(bad),
                    },
                    playback_evidence: match s.playback_evidence.as_str() {
                        "native" => PlaybackEvidence::NativeState,
                        "position" => PlaybackEvidence::PositionProgressRequired,
                        _ => return Err(bad),
                    },
                    media_id: s.media_id.clone(),
                    media_source_id: s.media_source_id.clone(),
                    position_ticks: s.position_ticks,
                    awake: s.awake,
                    volume_index: s.volume_index,
                    volume_max_index: s.volume_max_index,
                })
            })
            .transpose()?;
        let mut choices = Vec::new();
        for a in &self.choices {
            if a.id.len() > MAX_TEXT
                || a.component.len() > MAX_TEXT
                || a.label.as_ref().is_some_and(|v| v.len() > MAX_TEXT)
                || a.search_component
                    .as_ref()
                    .is_some_and(|v| v.len() > MAX_TEXT)
            {
                return Err(bad);
            }
            choices.push(App {
                id: a.id.clone(),
                component: a.component.clone(),
                label: a.label.clone(),
                search_component: a.search_component.clone(),
            });
        }
        Ok(Reply {
            request_id: self.request_id.clone(),
            status,
            code: self.code.clone(),
            observed,
            choices,
            cached: self.cached,
            local_elapsed_ms: self.local_elapsed_ms,
        })
    }
}
fn fail(code: &'static str) -> Failure {
    Failure::before(code)
}
fn private(file: &File) -> Result<(), &'static str> {
    let m = file.metadata().map_err(|_| "operation_journal_io")?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != 0o600
        || m.nlink() != 1
    {
        return Err("operation_journal_not_private");
    }
    Ok(())
}
fn open_at(
    directory: &File,
    name: &str,
    create: bool,
    exclusive: bool,
) -> Result<File, &'static str> {
    use std::os::fd::FromRawFd;
    let name = std::ffi::CString::new(name).map_err(|_| "operation_journal_io")?;
    let mut flags = libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
    if create {
        flags |= libc::O_CREAT;
    }
    if exclusive {
        flags |= libc::O_EXCL;
    }
    let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags, 0o600) };
    if fd < 0 {
        return Err(
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                "operation_journal_missing"
            } else {
                "operation_journal_io"
            },
        );
    }
    let f = unsafe { File::from_raw_fd(fd) };
    private(&f)?;
    Ok(f)
}
fn unlink_at(directory: &File, name: &str) {
    if let Ok(name) = std::ffi::CString::new(name) {
        unsafe {
            libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0);
        }
    }
}
fn locked(file: &File) -> Result<bool, &'static str> {
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::EWOULDBLOCK) => Ok(false),
        _ => Err("operation_journal_lock_failed"),
    }
}

impl OperationJournal {
    /// `path` is a private directory, not a journal file. Its parent must exist.
    pub fn open(path: &Path, capacity: usize, retention_ms: u64) -> Result<Self, &'static str> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
            || !(1..=4096).contains(&capacity)
            || !(1..=86_400_000).contains(&retention_ms)
        {
            return Err("operation_journal_configuration");
        }
        for p in path
            .parent()
            .ok_or("operation_journal_configuration")?
            .ancestors()
        {
            let m = p
                .symlink_metadata()
                .map_err(|_| "operation_journal_directory_missing")?;
            if !m.is_dir() || m.file_type().is_symlink() {
                return Err("operation_journal_symlink_refused");
            }
        }
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("operation_journal_io"),
        }
        let m = path
            .symlink_metadata()
            .map_err(|_| "operation_journal_io")?;
        if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o777 != 0o700 {
            return Err("operation_journal_not_private");
        }
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| "operation_journal_io")?;
        let actual = directory.metadata().map_err(|_| "operation_journal_io")?;
        if actual.dev() != m.dev() || actual.ino() != m.ino() {
            return Err("operation_journal_directory_changed");
        }
        let value = Self {
            directory,
            capacity,
            retention_ms,
            leases: Mutex::new(BTreeMap::new()),
        };
        let _lock = value.lock()?;
        let mut disk = value.load()?;
        if value.recover(&mut disk)? {
            value.save(&disk)?;
        }
        Ok(value)
    }
    fn lock(&self) -> Result<Transaction, &'static str> {
        let file = open_at(&self.directory, "transaction.lock", true, false)?;
        if !locked(&file)? {
            return Err("operation_journal_busy");
        }
        Ok(Transaction(file))
    }
    fn lease_name(&self, id: &str) -> String {
        format!("lease-{id}")
    }
    fn load(&self) -> Result<Disk, &'static str> {
        let file = match open_at(&self.directory, "operations.json", false, false) {
            Ok(file) => file,
            Err("operation_journal_missing") => return Ok(Disk::default()),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "operation_journal_io")?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("operation_journal_limit");
        }
        let disk: Disk = serde_json::from_slice(&bytes).map_err(|_| "operation_journal_corrupt")?;
        if disk.schema != 1 || disk.entries.len() > 4096 {
            return Err("operation_journal_corrupt");
        }
        for (id, e) in &disk.entries {
            if !tv_domain::identifier(id)
                || !tv_domain::identifier(&e.device)
                || e.resource.is_empty()
                || e.resource.len() > 512
                || e.reply.is_some() != e.completed_at.is_some()
            {
                return Err("operation_journal_corrupt");
            }
            if let Some(r) = &e.reply {
                if r.request_id != *id {
                    return Err("operation_journal_corrupt");
                }
                r.decode()?;
            }
            if serde_json::to_vec(&e.intent)
                .map_err(|_| "operation_journal_corrupt")?
                .len()
                > 16_384
            {
                return Err("operation_journal_corrupt");
            }
        }
        Ok(disk)
    }
    fn save(&self, disk: &Disk) -> Result<(), &'static str> {
        let bytes = serde_json::to_vec(disk).map_err(|_| "operation_journal_encode_failed")?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("operation_journal_limit");
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let tmp = format!(
            ".operations-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let mut f = open_at(&self.directory, &tmp, true, true)?;
        struct Cleanup<'a>(&'a File, String);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                unlink_at(self.0, &self.1);
            }
        }
        let _cleanup = Cleanup(&self.directory, tmp.clone());
        f.write_all(&bytes)
            .and_then(|_| f.sync_all())
            .map_err(|_| "operation_journal_io")?;
        let from = std::ffi::CString::new(tmp).map_err(|_| "operation_journal_io")?;
        let to = c"operations.json";
        if unsafe {
            libc::renameat(
                self.directory.as_raw_fd(),
                from.as_ptr(),
                self.directory.as_raw_fd(),
                to.as_ptr(),
            )
        } != 0
        {
            return Err("operation_journal_io");
        }
        self.directory
            .sync_all()
            .map_err(|_| "operation_journal_io")?;
        // All lease opens are serialized by transaction.lock. Terminal leases
        // cannot be in use, so unlinking here cannot split an active lock.
        for (id, entry) in &disk.entries {
            if entry.reply.is_some() {
                unlink_at(&self.directory, &self.lease_name(id));
            }
        }
        Ok(())
    }

    fn recover(&self, disk: &mut Disk) -> Result<bool, &'static str> {
        let mut changed = false;
        for (id, entry) in &mut disk.entries {
            if entry.reply.is_some() {
                continue;
            }
            let lease = open_at(&self.directory, &self.lease_name(id), true, false)?;
            if locked(&lease)? {
                entry.reply = Some(ReplyWire::from(&Reply {
                    request_id: id.clone(),
                    status: Status::OutcomeUnknown,
                    code: "operation_interrupted".into(),
                    observed: None,
                    choices: vec![],
                    cached: false,
                    local_elapsed_ms: 0,
                }));
                entry.completed_at = Some(0);
                changed = true;
                // Publication of the recovered record happens before cleanup.
            }
        }
        Ok(changed)
    }
    fn transaction<T>(
        &self,
        f: impl FnOnce(&mut Disk) -> Result<T, &'static str>,
    ) -> PortResult<T> {
        let _lock = self.lock().map_err(fail)?;
        let mut disk = self.load().map_err(fail)?;
        if self.recover(&mut disk).map_err(fail)? {
            self.save(&disk).map_err(fail)?;
        }
        f(&mut disk).map_err(fail)
    }
}

impl Journal for OperationJournal {
    fn reserve(&self, request: &Request, resource: &str, now: u64) -> PortResult<Reservation> {
        if !tv_domain::identifier(&request.id)
            || !tv_domain::identifier(&request.device)
            || resource.is_empty()
            || resource.len() > 512
        {
            return Err(fail("invalid_operation"));
        }
        let intent = IntentWire::from(&request.intent);
        if serde_json::to_vec(&intent)
            .map_err(|_| fail("invalid_operation"))?
            .len()
            > 16_384
        {
            return Err(fail("invalid_operation"));
        }
        self.transaction(|disk| {
            // Expiry is a dispatch budget, not part of an operation's identity.
            if let Some(e) = disk.entries.get(&request.id) {
                if e.device != request.device || e.intent != intent || e.resource != resource {
                    return Ok(Reservation::Conflict);
                }
                return Ok(Reservation::Existing(match &e.reply {
                    Some(r) => Operation::Complete(r.decode()?),
                    None => Operation::Running,
                }));
            }
            if disk
                .entries
                .values()
                .any(|e| e.resource == resource && e.reply.is_none())
            {
                return Ok(Reservation::Busy);
            }
            disk.entries.retain(|_, e| {
                e.reply
                    .as_ref()
                    .is_none_or(|r| r.status == "outcome_unknown")
                    || e.completed_at
                        .is_none_or(|at| now.saturating_sub(at) < self.retention_ms)
            });
            if disk.entries.len() >= self.capacity {
                return Err("operation_journal_capacity");
            }
            let lease = open_at(&self.directory, &self.lease_name(&request.id), true, false)?;
            if !locked(&lease)? {
                return Ok(Reservation::Busy);
            }
            lease.set_len(1).map_err(|_| "operation_journal_io")?;
            lease
                .write_all_at(&[0], 0)
                .and_then(|_| lease.sync_all())
                .map_err(|_| "operation_journal_io")?;
            let mut leases = self
                .leases
                .lock()
                .map_err(|_| "operation_journal_poisoned")?;
            disk.entries.insert(
                request.id.clone(),
                Entry {
                    device: request.device.clone(),
                    intent,
                    resource: resource.into(),
                    cancelled: false,
                    completed_at: None,
                    reply: None,
                },
            );
            self.save(disk)?;
            leases.insert(request.id.clone(), lease);
            Ok(Reservation::Acquired)
        })
    }
    fn finish(&self, request: &Request, reply: &Reply, now: u64) -> PortResult<()> {
        let result = self.transaction(|disk| {
            let mut leases = self
                .leases
                .lock()
                .map_err(|_| "operation_journal_poisoned")?;
            if !leases.contains_key(&request.id) {
                return Err("operation_not_owned");
            }
            let e = disk
                .entries
                .get_mut(&request.id)
                .ok_or("operation_not_found")?;
            if e.device != request.device
                || e.intent != IntentWire::from(&request.intent)
                || reply.request_id != request.id
                || e.reply.is_some()
            {
                return Err("operation_conflict");
            }
            let wire = ReplyWire::from(reply);
            wire.decode()?;
            e.reply = Some(wire);
            e.completed_at = Some(now);
            let saved = self.save(disk);
            // Failed publication cannot leave an immortal in-memory owner. The
            // next reader will recover the reservation as unknown if necessary.
            leases.remove(&request.id);
            saved
        });
        if result.is_err() {
            if let Ok(mut leases) = self.leases.lock() {
                leases.remove(&request.id);
            }
        }
        result
    }
    fn get(&self, id: &str) -> PortResult<Operation> {
        if !tv_domain::identifier(id) {
            return Err(fail("invalid_operation"));
        }
        self.transaction(|disk| match disk.entries.get(id) {
            None => Ok(Operation::Missing),
            Some(e) => match &e.reply {
                None => Ok(Operation::Running),
                Some(r) => Ok(Operation::Complete(r.decode()?)),
            },
        })
    }
    fn cancel(&self, id: &str) -> PortResult<Operation> {
        if !tv_domain::identifier(id) {
            return Err(fail("invalid_operation"));
        }
        self.transaction(|disk| {
            let Some(e) = disk.entries.get_mut(id) else {
                return Ok(Operation::Missing);
            };
            if let Some(r) = &e.reply {
                return Ok(Operation::Complete(r.decode()?));
            }
            let lease = open_at(&self.directory, &self.lease_name(id), false, false)?;
            lease
                .write_all_at(&[1], 0)
                .and_then(|_| lease.sync_all())
                .map_err(|_| "operation_journal_io")?;
            e.cancelled = true;
            self.save(disk)?;
            Ok(Operation::Running)
        })
    }
    fn is_cancelled(&self, id: &str) -> bool {
        // The hot path reads one byte through the held lease descriptor, without
        // locking or decoding the operation history on every process poll.
        let Ok(leases) = self.leases.lock() else {
            return true;
        };
        let Some(lease) = leases.get(id) else {
            return true;
        };
        let mut marker = [0u8];
        !matches!(lease.read_at(&mut marker, 0), Ok(1)) || marker[0] != 0
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::StateWire;

    #[test]
    fn pre_wake_state_has_unknown_power() {
        let state: StateWire = serde_json::from_str(r#"{"connected":true,"active_app":null,"playback":"unknown","playback_evidence":"native","media_id":null,"position_ticks":null,"volume_index":null}"#).unwrap();
        assert_eq!(state.awake, None);
    }
}
