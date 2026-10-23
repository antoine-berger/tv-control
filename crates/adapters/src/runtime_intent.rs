//! Runtime intent, published or explicitly replaced under the pipeline session lock.
use crate::pipeline_store::AtomicFile;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

const MAX_INTENT_BYTES: usize = 6_400_000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeGeneration {
    schema: u32,
    pub installation_id: String,
    pub revision: String,
    pub device: String,
    pub generation: String,
}
impl BridgeGeneration {
    fn validate(&self) -> Result<(), &'static str> {
        if self.schema != 1
            || ![
                &self.installation_id,
                &self.revision,
                &self.device,
                &self.generation,
            ]
            .iter()
            .all(|s| tv_domain::identifier(s))
        {
            return Err("bridge_generation_invalid");
        }
        Ok(())
    }
}

/// The bridge calls this only after acquiring its exclusive owned ADB runtime.
/// Every process incarnation gets a fresh token, including supervisor restarts.
pub fn start_generation(
    directory: &Path,
    installation_id: &str,
    revision: &str,
    device: &str,
) -> Result<BridgeGeneration, &'static str> {
    private_directory(directory)?;
    let generation = BridgeGeneration {
        schema: 1,
        installation_id: installation_id.into(),
        revision: revision.into(),
        device: device.into(),
        generation: crate::connection_proof::random_token()?,
    };
    generation.validate()?;
    let mut file = AtomicFile::open(directory.join("bridge-generation.json"), 8192)?;
    if let Some(bytes) = file.bytes() {
        let previous: BridgeGeneration =
            serde_json::from_slice(bytes).map_err(|_| "bridge_generation_invalid")?;
        previous.validate()?;
        if previous.installation_id != installation_id {
            return Err("bridge_generation_installation_mismatch");
        }
    }
    file.write(serde_json::to_vec(&generation).map_err(|_| "bridge_generation_encode_failed")?)?;
    Ok(generation)
}

pub fn read_generation(
    directory: &Path,
    installation_id: &str,
    revision: &str,
    device: &str,
) -> Result<String, &'static str> {
    private_directory(directory)?;
    // Reading while the bridge publishes is intentional. Do not use AtomicFile's
    // interrupted-write recovery here: that would remove a live writer's temp link.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(directory.join("bridge-generation.json"))
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "bridge_generation_unavailable"
            } else {
                "bridge_generation_invalid"
            }
        })?;
    let metadata = file.metadata().map_err(|_| "bridge_generation_invalid")?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o600
        || metadata.len() > 8192
    {
        return Err("bridge_generation_invalid");
    }
    if metadata.nlink() == 2 {
        return Err("bridge_generation_pending");
    }
    if metadata.nlink() != 1 {
        return Err("bridge_generation_invalid");
    }
    let mut bytes = Vec::new();
    file.take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| "bridge_generation_invalid")?;
    if bytes.len() > 8192 {
        return Err("bridge_generation_invalid");
    }
    let generation: BridgeGeneration =
        serde_json::from_slice(&bytes).map_err(|_| "bridge_generation_invalid")?;
    generation.validate()?;
    if generation.installation_id != installation_id
        || generation.revision != revision
        || generation.device != device
    {
        return Err("bridge_generation_binding_mismatch");
    }
    Ok(generation.generation)
}

fn private_directory(directory: &Path) -> Result<(), &'static str> {
    let metadata =
        fs::symlink_metadata(directory).map_err(|_| "runtime_intent_directory_not_private")?;
    if !directory.is_absolute()
        || !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o700
    {
        return Err("runtime_intent_directory_not_private");
    }
    Ok(())
}

/// The directory must already be owned by the current user, private and locked
/// through `pipeline_store::Store`. Existing different data is never replaced.
pub fn publish(directory: &Path, bytes: &[u8]) -> Result<(), &'static str> {
    if bytes.len() > MAX_INTENT_BYTES {
        return Err("runtime_intent_too_large");
    }
    private_directory(directory)?;
    let mut file = AtomicFile::open(directory.join("runtime-intent.json"), MAX_INTENT_BYTES)?;
    match file.bytes() {
        Some(existing) if existing == bytes => Ok(()),
        Some(_) => Err("runtime_intent_conflict"),
        None => file.write(bytes.to_vec()),
    }
}

/// Replace an explicitly reapproved configuration only after its exact owned
/// runtime has been verified stopped. The caller must hold the session lock.
/// Preserve the old intent before the atomic replacement; interruption leaves
/// either the old or new complete intent, never a missing binding.
pub fn replace_after_stop(
    directory: &Path,
    expected: &[u8],
    replacement: &[u8],
) -> Result<PathBuf, &'static str> {
    private_directory(directory)?;
    if expected.len() > MAX_INTENT_BYTES || replacement.len() > MAX_INTENT_BYTES {
        return Err("runtime_intent_too_large");
    }
    let path = directory.join("runtime-intent.json");
    let mut current = AtomicFile::open(path, MAX_INTENT_BYTES)?;
    if current.bytes() != Some(expected) {
        return Err("runtime_intent_conflict");
    }
    let backup = directory.join(format!(
        "runtime-intent.previous-{}.json",
        crate::connection_proof::random_token()?
    ));
    let mut preserved = AtomicFile::open(backup.clone(), MAX_INTENT_BYTES)?;
    if preserved.bytes().is_some() {
        return Err("runtime_intent_backup_conflict");
    }
    preserved.write(expected.to_vec())?;
    current.write(replacement.to_vec())?;
    Ok(backup)
}
