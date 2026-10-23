//! Explicitly consented, installation-bound runtime credential storage.
//!
//! This is a private plaintext file, not an OS keychain. Callers must say so
//! when requesting persistence consent and hold the pipeline Store session lock
//! while saving. Credentials must never be included in diagnostics or argv.
use crate::pipeline_store::AtomicFile;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

const FILE: &str = "runtime-credential.json";
const LIMIT: usize = 32_768;
const KEY_LIMIT: usize = 4096;

// Deliberately not Debug: the record contains a secret.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Credential {
    schema: u32,
    installation_id: String,
    tunnel_id: String,
    key: String,
}

fn binding(installation: &str, tunnel: &str) -> Result<(), &'static str> {
    if !tv_domain::identifier(installation)
        || !tunnel.starts_with("tunnel_")
        || tunnel.len() > 128
        || !tunnel
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err("runtime_credential_binding_invalid");
    }
    Ok(())
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= KEY_LIMIT && !key.chars().any(char::is_control)
}

fn directory(state: &Path) -> Result<(), &'static str> {
    if !state.is_absolute() {
        return Err("runtime_credential_directory_not_private");
    }
    for ancestor in state.ancestors() {
        let m = fs::symlink_metadata(ancestor)
            .map_err(|_| "runtime_credential_directory_not_private")?;
        if !m.is_dir() {
            return Err("runtime_credential_directory_not_private");
        }
    }
    let m = fs::symlink_metadata(state).map_err(|_| "runtime_credential_directory_not_private")?;
    if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o7777 != 0o700 {
        return Err("runtime_credential_directory_not_private");
    }
    Ok(())
}

/// Side-effect-free read. Missing storage is not proof that a running controller
/// lacks its in-memory key. Unsafe or mismatched storage is never treated as absent.
pub fn load(
    state: &Path,
    installation: &str,
    tunnel: &str,
) -> Result<Option<String>, &'static str> {
    binding(installation, tunnel)?;
    directory(state)?;
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(state.join(FILE))
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("runtime_credential_unreadable"),
    };
    let m = file
        .metadata()
        .map_err(|_| "runtime_credential_unreadable")?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o7777 != 0o600
        || m.nlink() != 1
        || m.len() > LIMIT as u64
    {
        return Err("runtime_credential_not_private");
    }
    let mut bytes = Vec::new();
    file.take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "runtime_credential_unreadable")?;
    if bytes.len() > LIMIT {
        return Err("runtime_credential_invalid");
    }
    let saved: Credential =
        serde_json::from_slice(&bytes).map_err(|_| "runtime_credential_invalid")?;
    if saved.schema != 1 || !valid_key(&saved.key) {
        return Err("runtime_credential_invalid");
    }
    if saved.installation_id != installation || saved.tunnel_id != tunnel {
        return Err("runtime_credential_binding_mismatch");
    }
    Ok(Some(saved.key))
}

/// Save only after explicit private-plaintext-storage consent. Refuses insecure,
/// corrupt or differently bound existing records; never repairs permissions.
pub fn save(
    state: &Path,
    installation: &str,
    tunnel: &str,
    key: &str,
    approved: bool,
) -> Result<(), &'static str> {
    if !approved {
        return Err("runtime_credential_storage_approval_required");
    }
    if !valid_key(key) {
        return Err("runtime_credential_invalid");
    }
    load(state, installation, tunnel)?;
    let record = Credential {
        schema: 1,
        installation_id: installation.into(),
        tunnel_id: tunnel.into(),
        key: key.into(),
    };
    let bytes = serde_json::to_vec(&record).map_err(|_| "runtime_credential_encode_failed")?;
    let mut file = AtomicFile::open(state.join(FILE), LIMIT)?;
    file.write(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "tv-credential-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            Self(root)
        }
        fn save(&self) {
            save(
                &self.0,
                "installation",
                "tunnel_fixture",
                "fixture-only-not-a-credential",
                true,
            )
            .unwrap();
        }
        fn load(&self) -> Result<Option<String>, &'static str> {
            load(&self.0, "installation", "tunnel_fixture")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_and_declined_storage_have_no_side_effects() {
        let f = Fixture::new();
        assert_eq!(f.load(), Ok(None));
        assert!(save(
            &f.0,
            "installation",
            "tunnel_fixture",
            "fixture-only",
            false
        )
        .is_err());
        assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
    }

    #[test]
    fn saves_private_bound_record_and_reuses_it() {
        let f = Fixture::new();
        f.save();
        assert_eq!(
            f.load().unwrap().as_deref(),
            Some("fixture-only-not-a-credential")
        );
        let m = fs::metadata(f.0.join(FILE)).unwrap();
        assert_eq!(m.mode() & 0o7777, 0o600);
        assert_eq!(m.nlink(), 1);
        assert_eq!(
            load(&f.0, "other", "tunnel_fixture"),
            Err("runtime_credential_binding_mismatch")
        );
        assert_eq!(
            load(&f.0, "installation", "tunnel_other"),
            Err("runtime_credential_binding_mismatch")
        );
        assert!(save(&f.0, "other", "tunnel_fixture", "fixture-only", true).is_err());
        assert_eq!(
            f.load().unwrap().as_deref(),
            Some("fixture-only-not-a-credential")
        );
    }

    #[test]
    fn same_binding_rotation_is_atomic_and_private() {
        let f = Fixture::new();
        f.save();
        save(
            &f.0,
            "installation",
            "tunnel_fixture",
            "replacement-fixture-only",
            true,
        )
        .unwrap();
        assert_eq!(
            f.load().unwrap().as_deref(),
            Some("replacement-fixture-only")
        );
        let m = fs::metadata(f.0.join(FILE)).unwrap();
        assert_eq!(m.mode() & 0o7777, 0o600);
        assert_eq!(m.nlink(), 1);
        assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
    }

    #[test]
    fn rejects_symlink_ancestor_without_touching_target() {
        let f = Fixture::new();
        let nested = f.0.join("private");
        fs::create_dir(&nested).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o700)).unwrap();
        let link = f.0.join("alias");
        symlink(&f.0, &link).unwrap();
        let through_link = link.join("private");
        assert_eq!(
            load(&through_link, "installation", "tunnel_fixture"),
            Err("runtime_credential_directory_not_private")
        );
        assert!(save(
            &through_link,
            "installation",
            "tunnel_fixture",
            "fixture-only",
            true
        )
        .is_err());
        assert_eq!(fs::read_dir(&nested).unwrap().count(), 0);
    }

    #[test]
    fn rejects_links_and_does_not_repair_permissions() {
        let f = Fixture::new();
        f.save();
        let path = f.0.join(FILE);
        fs::hard_link(&path, f.0.join("other")).unwrap();
        assert!(f.load().is_err());
        assert!(save(&f.0, "installation", "tunnel_fixture", "fixture-only", true).is_err());
        assert_eq!(fs::metadata(&path).unwrap().nlink(), 2);
        fs::remove_file(f.0.join("other")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(f.load().is_err());
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o644);
        fs::remove_file(&path).unwrap();
        symlink("missing", &path).unwrap();
        assert!(f.load().is_err());
    }

    #[test]
    fn rejects_unsafe_directory_corrupt_or_oversized_record_and_invalid_key() {
        let f = Fixture::new();
        for key in [
            String::new(),
            "fixture\nonly".into(),
            "x".repeat(KEY_LIMIT + 1),
        ] {
            assert!(save(&f.0, "installation", "tunnel_fixture", &key, true).is_err());
        }
        f.save();
        let path = f.0.join(FILE);
        fs::write(&path, b"not json").unwrap();
        assert_eq!(f.load(), Err("runtime_credential_invalid"));
        fs::write(&path, vec![b'x'; LIMIT + 1]).unwrap();
        assert_eq!(f.load(), Err("runtime_credential_not_private"));
        fs::set_permissions(&f.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(f.load(), Err("runtime_credential_directory_not_private"));
    }
}
