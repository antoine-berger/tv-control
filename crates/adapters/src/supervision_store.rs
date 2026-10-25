//! Private, revision-fenced host supervision state; never stores credentials.
mod context;
mod files;
mod wire;

pub use context::ContextStore;

use files::{Directory, FileLock};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};
use tv_application::supervision::{Mode, Phase, Record, Registration, Store};
use tv_application::Context;
use tv_domain::{Failure, PortResult};

pub const DIRECTORY: &str = "supervision";
const STATE: &str = "state.json";
const TRANSACTION: &str = "transaction.lock";
const OWNER: &str = "owner.lock";
const MAX_BYTES: usize = 128 * 1024;

fn fail(code: &'static str) -> Failure {
    Failure::before(code)
}
fn clean_path(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
        && path
            .to_str()
            .is_some_and(|v| !v.is_empty() && v.len() <= 4096 && !v.chars().any(char::is_control))
}

/// An existing access binding. `pins` contains only SHA-256 fingerprints indexed
/// by absolute filename; credential/key contents must never be copied here.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub installation_id: String,
    pub tunnel_id: String,
    pub configuration: PathBuf,
    pub launcher: PathBuf,
    pub helper: PathBuf,
    pub executable_path: String,
    pub label: String,
    pub artifact: PathBuf,
    pub uid: u32,
    pub pins: BTreeMap<String, String>,
}
impl Binding {
    pub fn validate(&self) -> PortResult<()> {
        let invalid = || fail("supervision_binding_invalid");
        if !tv_domain::identifier(&self.installation_id)
            || !self.tunnel_id.starts_with("tunnel_")
            || !(8..=128).contains(&self.tunnel_id.len())
            || !self
                .tunnel_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || self.uid == 0
            || self.uid == u32::MAX
            || !(1..=128).contains(&self.label.len())
            || !self.label.as_bytes()[0].is_ascii_alphanumeric()
            || !self
                .label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || [
                &self.configuration,
                &self.launcher,
                &self.helper,
                &self.artifact,
            ]
            .iter()
            .any(|p| !clean_path(p))
            || self.executable_path.is_empty()
            || self.executable_path.len() > 16_384
            || self
                .executable_path
                .split(':')
                .any(|p| !clean_path(Path::new(p)))
            || !(1..=32).contains(&self.pins.len())
            || self.pins.keys().map(String::len).sum::<usize>() > 32_768
        {
            return Err(invalid());
        }
        for (path, digest) in &self.pins {
            if !clean_path(Path::new(path))
                || digest.len() != 64
                || !digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

/// Opening is read-only: it neither creates state/locks nor repairs unsafe files.
/// The cached binding is immutable for the lifetime of this handle.
pub struct SupervisionStore {
    directory: Directory,
    binding: Binding,
}

/// Keep this value across the entire supervisor incarnation or explicit stop
/// cleanup. The transaction lock is separate and is never held by this guard.
pub struct OwnerLease {
    _guard: FileLock,
}

impl SupervisionStore {
    pub fn with_context<'a, 'ctx>(&'a self, ctx: &'a Context<'ctx>) -> ContextStore<'a, 'ctx> {
        ContextStore::new(self, ctx)
    }

    pub fn open_with_context(connection: &Path, ctx: &Context<'_>) -> PortResult<Self> {
        context::retry(ctx, || Self::open(connection))
    }

    pub fn load_with_context(&self, ctx: &Context<'_>) -> PortResult<Record> {
        self.with_context(ctx).load()
    }

    pub fn binding_with_context(&self, ctx: &Context<'_>) -> PortResult<Binding> {
        context::retry(ctx, || self.binding())
    }

    pub fn try_owner_with_context(&self, ctx: &Context<'_>) -> PortResult<OwnerLease> {
        context::retry(ctx, || self.try_owner())
    }

    pub fn prepare(connection: &Path, binding: &Binding, approved: bool) -> PortResult<Self> {
        if !approved {
            return Err(fail("supervision_approval_required"));
        }
        binding.validate()?;
        let directory = Directory::prepare(connection)?;
        directory.create(TRANSACTION, &[])?;
        directory.create(OWNER, &[])?;
        let bytes = wire::encode(binding, &Record::prepared())?;
        directory.create(STATE, &bytes)?;
        Self::open(connection)
    }

    pub fn open(connection: &Path) -> PortResult<Self> {
        let directory = Directory::open(connection)?;
        let _transaction = directory.lock(TRANSACTION, "supervision_store_busy")?;
        let (binding, _) = wire::decode(&directory.read(STATE, MAX_BYTES)?)?;
        Ok(Self { directory, binding })
    }

    pub fn binding(&self) -> PortResult<Binding> {
        let _transaction = self.directory.lock(TRANSACTION, "supervision_store_busy")?;
        let (binding, _) = self.snapshot()?;
        Ok(binding)
    }

    pub fn directory(&self) -> &Path {
        self.directory.path()
    }

    pub fn try_owner(&self) -> PortResult<OwnerLease> {
        self.directory.check()?;
        // Validate state without retaining the short transaction lock.
        self.load()?;
        self.directory
            .lock(OWNER, "supervision_owner_busy")
            .map(|guard| OwnerLease { _guard: guard })
    }

    /// Remove only a verified Removed store. The caller must already have an
    /// acknowledged OS unload and removal of its owned service artifact.
    /// Partial filesystem failure remains uncertain; it never repairs/adopts
    /// remaining entries or removes configuration, identities or the journal.
    pub fn purge_removed(&self, owner: &OwnerLease) -> PortResult<()> {
        self.directory.check_owner(&owner._guard)?;
        let transaction = self.directory.lock(TRANSACTION, "supervision_store_busy")?;
        let (_, record) = self.snapshot()?;
        if record.registration != Registration::Removed
            || record.mode != Mode::Stopped
            || record.phase != Phase::Idle
        {
            return Err(fail("supervision_purge_not_removed"));
        }
        match self.binding.artifact.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(fail("supervision_artifact_removal_unverified")),
        }
        self.directory.purge(&owner._guard, &transaction)
    }

    fn snapshot(&self) -> PortResult<(Binding, Record)> {
        let (binding, record) = wire::decode(&self.directory.read(STATE, MAX_BYTES)?)?;
        if binding != self.binding {
            return Err(fail("supervision_binding_changed"));
        }
        Ok((binding, record))
    }
}
impl Store for SupervisionStore {
    fn load(&self) -> PortResult<Record> {
        let _transaction = self.directory.lock(TRANSACTION, "supervision_store_busy")?;
        self.snapshot().map(|(_, record)| record)
    }

    fn compare_and_set(&self, expected_revision: u64, next: &Record) -> PortResult<bool> {
        next.validate()?;
        if expected_revision.checked_add(1) != Some(next.revision) {
            return Err(fail("supervision_revision_invalid"));
        }
        let _transaction = self.directory.lock(TRANSACTION, "supervision_store_busy")?;
        let (binding, current) = self.snapshot()?;
        if current.revision != expected_revision {
            return Ok(false);
        }
        if next.arm < current.arm
            || (next.arm != current.arm && current.arm.checked_add(1) != Some(next.arm))
            || (next.arm == current.arm
                && (next.starts < current.starts || next.unclean < current.unclean))
        {
            return Err(fail("supervision_state_rollback_refused"));
        }
        self.directory
            .replace(STATE, &wire::encode(&binding, next)?)?;
        Ok(true)
    }
}
