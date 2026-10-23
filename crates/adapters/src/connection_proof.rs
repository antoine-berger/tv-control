//! Ephemeral, cooperative verification of a call made through the selected chat.
//! This is not cryptographic attestation of a conversation or an account credential.
use serde::{Deserialize, Serialize};
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};
use tv_domain::{Observed, Receipt, Status};

pub const CHALLENGE_VALIDITY_MS: u64 = 120_000;
const MAX_SNAPSHOT: usize = 1_048_576;
const MAX_FILE: u64 = 6_400_000;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Challenge {
    pub nonce: String,
    pub installation_id: String,
    pub configuration_revision: String,
    pub device: String,
    pub bridge_generation: String,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestFile {
    schema: u32,
    challenge: Challenge,
    snapshot: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseFile {
    schema: u32,
    nonce: String,
    bridge_generation: String,
    code: String,
    observed_at_unix_ms: u64,
    local_elapsed_ms: u64,
}

/// Return this code only in the MCP response, never a local log or checkpoint.
pub struct VerificationCode {
    pub code: String,
    pub installation_id: String,
    pub configuration_revision: String,
}
pub struct Confirmation {
    pub challenge: Challenge,
    pub observed_at_unix_ms: u64,
    /// Privacy-preserving projection of the actual successful Observe receipt.
    pub receipt: Receipt,
}
pub struct ProofStore {
    path: PathBuf,
    directory: File,
}
struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

pub fn random_token() -> Result<String, &'static str> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|_| "proof_random_unavailable")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn token(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn private_file(file: &File) -> Result<fs::Metadata, &'static str> {
    let metadata = file.metadata().map_err(|_| "proof_file_unreadable")?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.nlink() != 1
    {
        return Err("proof_file_not_private");
    }
    Ok(metadata)
}
impl ProofStore {
    /// The parent must itself be a trusted private application directory.
    pub fn open(path: &Path) -> Result<Self, &'static str> {
        if !path.is_absolute() {
            return Err("proof_directory_not_absolute");
        }
        let parent = path.parent().ok_or("proof_directory_invalid")?;
        let metadata = fs::symlink_metadata(parent).map_err(|_| "proof_parent_unavailable")?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err("proof_parent_not_private");
        }
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("proof_directory_unavailable"),
        }
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(path)
            .map_err(|_| "proof_directory_unavailable")?;
        let metadata = directory
            .metadata()
            .map_err(|_| "proof_directory_unavailable")?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o777 != 0o700
        {
            return Err("proof_directory_not_private");
        }
        Ok(Self {
            path: path.into(),
            directory,
        })
    }
    fn file(&self, name: &str, flags: i32) -> Result<File, &'static str> {
        let name = CString::new(name).map_err(|_| "proof_filename_invalid")?;
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                    "proof_not_found"
                } else {
                    "proof_file_unavailable"
                },
            );
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    fn remove(&self, name: &str) -> Result<(), &'static str> {
        let name = CString::new(name).map_err(|_| "proof_filename_invalid")?;
        if unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err("proof_cleanup_failed");
        }
        Ok(())
    }
    fn lock(&self) -> Result<Lock, &'static str> {
        let current =
            fs::symlink_metadata(&self.path).map_err(|_| "proof_directory_unavailable")?;
        let held = self
            .directory
            .metadata()
            .map_err(|_| "proof_directory_unavailable")?;
        if !current.is_dir() || (current.dev(), current.ino()) != (held.dev(), held.ino()) {
            return Err("proof_directory_changed");
        }
        let file = self.file(".lock", libc::O_RDWR | libc::O_CREAT)?;
        private_file(&file)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("proof_busy");
        }
        let guard = Lock(file);
        self.recover()?;
        Ok(guard)
    }
    fn recover(&self) -> Result<(), &'static str> {
        let mut names = Vec::new();
        for item in fs::read_dir(&self.path)
            .map_err(|_| "proof_directory_unavailable")?
            .take(9)
        {
            let item = item.map_err(|_| "proof_directory_unavailable")?;
            let name = item
                .file_name()
                .into_string()
                .map_err(|_| "proof_unknown_entry")?;
            if names.len() >= 8 {
                return Err("proof_file_limit");
            }
            if name != ".lock"
                && name != "request.json"
                && name != "response.json"
                && !name
                    .strip_prefix(".proof-")
                    .and_then(|n| n.strip_suffix(".tmp"))
                    .is_some_and(token)
            {
                return Err("proof_unknown_entry");
            }
            names.push(name);
        }
        for name in &names {
            if !name.starts_with(".proof-") {
                continue;
            }
            let file = self.file(name, libc::O_RDONLY)?;
            let m = file.metadata().map_err(|_| "proof_file_unreadable")?;
            if !m.is_file()
                || m.uid() != unsafe { libc::geteuid() }
                || m.permissions().mode() & 0o777 != 0o600
                || m.nlink() > 2
            {
                return Err("proof_file_not_private");
            }
            if m.nlink() == 2 {
                let linked = ["request.json", "response.json"].iter().any(|target| {
                    self.file(target, libc::O_RDONLY)
                        .ok()
                        .and_then(|f| f.metadata().ok())
                        .is_some_and(|other| (m.dev(), m.ino()) == (other.dev(), other.ino()))
                });
                if !linked {
                    return Err("proof_unknown_link");
                }
            }
            self.remove(name)?;
        }
        for name in ["request.json", "response.json"] {
            match self.file(name, libc::O_RDONLY) {
                Ok(file) => {
                    private_file(&file)?;
                }
                Err("proof_not_found") => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    fn read<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Result<T, &'static str> {
        let file = self.file(name, libc::O_RDONLY)?;
        let metadata = private_file(&file)?;
        if metadata.len() > MAX_FILE {
            return Err("proof_file_limit");
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "proof_file_unreadable")?;
        if bytes.len() as u64 > MAX_FILE {
            return Err("proof_file_limit");
        }
        serde_json::from_slice(&bytes).map_err(|_| "proof_corrupt")
    }
    fn publish<T: Serialize>(&self, name: &str, value: &T) -> Result<(), &'static str> {
        let temp = format!(".proof-{}.tmp", random_token()?);
        let mut file = self.file(&temp, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
        let result = (|| {
            let bytes = serde_json::to_vec(value).map_err(|_| "proof_encode_failed")?;
            if bytes.len() as u64 > MAX_FILE {
                return Err("proof_file_limit");
            }
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| "proof_write_failed")?;
            let old = CString::new(temp.as_str()).unwrap();
            let new = CString::new(name).unwrap();
            if unsafe {
                libc::linkat(
                    self.directory.as_raw_fd(),
                    old.as_ptr(),
                    self.directory.as_raw_fd(),
                    new.as_ptr(),
                    0,
                )
            } != 0
            {
                return Err("proof_already_exists");
            }
            self.directory
                .sync_all()
                .map_err(|_| "proof_write_failed")?;
            Ok(())
        })();
        let cleanup = self.remove(&temp);
        result.and(cleanup)
    }
    fn checked(
        &self,
        nonce: &str,
        config: &[u8],
        generation: &str,
        now: u64,
    ) -> Result<RequestFile, &'static str> {
        if !token(nonce) {
            return Err("proof_invalid_challenge");
        }
        let request: RequestFile = self.read("request.json")?;
        if request.schema != 1
            || request.challenge.nonce != nonce
            || !token(&request.challenge.nonce)
            || !tv_domain::identifier(&request.challenge.installation_id)
            || !tv_domain::identifier(&request.challenge.configuration_revision)
            || !tv_domain::identifier(&request.challenge.device)
            || !tv_domain::identifier(&request.challenge.bridge_generation)
        {
            return Err("proof_challenge_mismatch");
        }
        if request.challenge.issued_at_unix_ms > now
            || now >= request.challenge.expires_at_unix_ms
            || request
                .challenge
                .expires_at_unix_ms
                .saturating_sub(request.challenge.issued_at_unix_ms)
                != CHALLENGE_VALIDITY_MS
        {
            self.remove("request.json")?;
            match self.file("response.json", libc::O_RDONLY) {
                Ok(_) => self.remove("response.json")?,
                Err("proof_not_found") => {}
                Err(e) => return Err(e),
            }
            return Err("proof_expired");
        }
        if request.snapshot.as_bytes() != config {
            return Err("proof_configuration_changed");
        }
        if request.challenge.bridge_generation != generation {
            return Err("proof_runtime_changed");
        }
        Ok(request)
    }
    pub fn issue(
        &self,
        installation_id: &str,
        revision: &str,
        config: &[u8],
        device: &str,
        generation: &str,
        now: u64,
    ) -> Result<Challenge, &'static str> {
        if !tv_domain::identifier(installation_id)
            || !tv_domain::identifier(revision)
            || !tv_domain::identifier(device)
            || !tv_domain::identifier(generation)
            || config.len() > MAX_SNAPSHOT
        {
            return Err("proof_invalid_binding");
        }
        let snapshot = std::str::from_utf8(config)
            .map_err(|_| "proof_invalid_configuration")?
            .to_string();
        let _lock = self.lock()?;
        // A new local journey deliberately invalidates its predecessor; no old proof is resumed.
        for name in ["request.json", "response.json"] {
            match self.file(name, libc::O_RDONLY) {
                Ok(_) => self.remove(name)?,
                Err("proof_not_found") => {}
                Err(e) => return Err(e),
            }
        }
        let challenge = Challenge {
            nonce: random_token()?,
            installation_id: installation_id.into(),
            configuration_revision: revision.into(),
            device: device.into(),
            bridge_generation: generation.into(),
            issued_at_unix_ms: now,
            expires_at_unix_ms: now
                .checked_add(CHALLENGE_VALIDITY_MS)
                .ok_or("proof_invalid_time")?,
        };
        self.publish(
            "request.json",
            &RequestFile {
                schema: 1,
                challenge: challenge.clone(),
                snapshot,
            },
        )?;
        Ok(challenge)
    }
    pub fn request(
        &self,
        nonce: &str,
        config: &[u8],
        generation: &str,
        now: u64,
    ) -> Result<Challenge, &'static str> {
        let _lock = self.lock()?;
        Ok(self.checked(nonce, config, generation, now)?.challenge)
    }
    /// Invalidate all pending evidence after an approved configuration transition.
    /// Unknown entries, unsafe links and nonprivate files are still refused.
    pub fn invalidate(&self) -> Result<(), &'static str> {
        let _lock = self.lock()?;
        for name in ["request.json", "response.json"] {
            match self.file(name, libc::O_RDONLY) {
                Ok(_) => self.remove(name)?,
                Err("proof_not_found") => {}
                Err(e) => return Err(e),
            }
        }
        self.directory
            .sync_all()
            .map_err(|_| "proof_cleanup_failed")
    }
    pub fn complete(
        &self,
        nonce: &str,
        config: &[u8],
        generation: &str,
        receipt: &Receipt,
        now: u64,
    ) -> Result<VerificationCode, &'static str> {
        let _lock = self.lock()?;
        let request = self.checked(nonce, config, generation, now)?;
        if receipt.request_id != nonce
            || receipt.status != Status::Observed
            || receipt.cached
            || !receipt
                .observed
                .as_ref()
                .is_some_and(|state| state.connected)
        {
            return Err("proof_observation_required");
        }
        let code = random_token()?;
        self.publish(
            "response.json",
            &ResponseFile {
                schema: 1,
                nonce: nonce.into(),
                bridge_generation: generation.into(),
                code: code.clone(),
                observed_at_unix_ms: now,
                local_elapsed_ms: receipt.local_elapsed_ms,
            },
        )?;
        Ok(VerificationCode {
            code,
            installation_id: request.challenge.installation_id,
            configuration_revision: request.challenge.configuration_revision,
        })
    }
    pub fn confirm(
        &self,
        challenge: &Challenge,
        returned_code: &str,
        config: &[u8],
        now: u64,
    ) -> Result<Confirmation, &'static str> {
        let _lock = self.lock()?;
        let request = self.checked(&challenge.nonce, config, &challenge.bridge_generation, now)?;
        if &request.challenge != challenge {
            return Err("proof_binding_mismatch");
        }
        let response: ResponseFile = self.read("response.json")?;
        if response.schema != 1
            || response.nonce != challenge.nonce
            || response.bridge_generation != challenge.bridge_generation
            || !token(&response.code)
            || response.observed_at_unix_ms < challenge.issued_at_unix_ms
            || response.observed_at_unix_ms > now
        {
            return Err("proof_invalid_response");
        }
        let equal = returned_code.len() == response.code.len()
            && returned_code
                .bytes()
                .zip(response.code.bytes())
                .fold(0u8, |diff, (a, b)| diff | (a ^ b))
                == 0;
        if !equal {
            return Err("proof_return_code_required");
        }
        // Removing the request first makes a crash during cleanup fail closed on replay.
        self.remove("request.json")?;
        self.remove("response.json")?;
        Ok(Confirmation {
            challenge: challenge.clone(),
            observed_at_unix_ms: response.observed_at_unix_ms,
            receipt: Receipt {
                request_id: challenge.nonce.clone(),
                status: Status::Observed,
                code: "connection_observed",
                observed: Some(Observed {
                    connected: true,
                    ..Observed::default()
                }),
                local_elapsed_ms: response.local_elapsed_ms,
                cached: false,
            },
        })
    }
    pub fn cancel(&self, challenge: &Challenge) -> Result<(), &'static str> {
        let _lock = self.lock()?;
        let request: RequestFile = self.read("request.json")?;
        if request.challenge != *challenge {
            return Err("proof_binding_mismatch");
        }
        self.remove("request.json")?;
        match self.file("response.json", libc::O_RDONLY) {
            Ok(_) => self.remove("response.json"),
            Err("proof_not_found") => Ok(()),
            Err(e) => Err(e),
        }
    }
}

use std::os::unix::fs::DirBuilderExt;
