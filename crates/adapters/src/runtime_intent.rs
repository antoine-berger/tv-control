//! Immutable runtime intent, published while the caller holds the pipeline session lock.
use crate::pipeline_store::AtomicFile;
use std::{fs, os::unix::fs::MetadataExt, path::Path};

const MAX_INTENT_BYTES: usize = 6_400_000;

/// The directory must already be owned by the current user, private and locked
/// through `pipeline_store::Store`. Existing different data is never replaced.
pub fn publish(directory: &Path, bytes: &[u8]) -> Result<(), &'static str> {
    if bytes.len() > MAX_INTENT_BYTES {
        return Err("runtime_intent_too_large");
    }
    let metadata =
        fs::symlink_metadata(directory).map_err(|_| "runtime_intent_directory_not_private")?;
    if !directory.is_absolute()
        || !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o700
    {
        return Err("runtime_intent_directory_not_private");
    }
    let mut file = AtomicFile::open(directory.join("runtime-intent.json"), MAX_INTENT_BYTES)?;
    match file.bytes() {
        Some(existing) if existing == bytes => Ok(()),
        Some(_) => Err("runtime_intent_conflict"),
        None => file.write(bytes.to_vec()),
    }
}
