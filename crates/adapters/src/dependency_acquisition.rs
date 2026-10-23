//! Dependency acquisition only: never authenticates, starts a tunnel, or runs the payload.
//! Linux archives are pinned to release digests. macOS uses the upstream-supported
//! Homebrew formula; it deliberately has a separate, explicit consent boundary.
use crate::process::{Invocation, Process};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};
use tv_application::Context;

pub const VERSION: &str = "v0.0.17";
const ROOT: &str = "https://github.com/openai/tunnel-client/releases/download";
const MAX_ARCHIVE: u64 = 32 * 1024 * 1024;
const MAX_MEMBER: usize = 64 * 1024 * 1024;
const OWNER: &str = "TV_CONTROL_TUNNEL_DEPENDENCY_V1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Package {
    Tunnel,
    Adb,
}

#[derive(Clone, Copy, Debug)]
pub struct Release {
    package: Package,
    platform: &'static str,
    sha256: &'static str,
}
impl Release {
    pub fn linux(arch: &str) -> Result<Self, &'static str> {
        match arch {
            "x86_64" => Ok(Self {
                package: Package::Tunnel,
                platform: "linux-amd64",
                sha256: "bd6a57c6c0511644267cafadd13ffdb53ece0d753e0b88857f957e78a8fbc793",
            }),
            "aarch64" => Ok(Self {
                package: Package::Tunnel,
                platform: "linux-arm64",
                sha256: "f5060d0b63663268aab3ca310cb6110fa14867981db2b0657f679d8836a2daa0",
            }),
            _ => Err("tunnel_dependency_platform_unsupported"),
        }
    }
    pub fn adb_linux(arch: &str) -> Result<Self, &'static str> {
        if arch != "x86_64" {
            return Err("adb_official_archive_platform_unsupported");
        }
        Ok(Self {
            package: Package::Adb,
            platform: "linux-amd64",
            sha256: "d230f13842f60f782a8645f9c813f8f845bf36089ea7289f28c48f17979313f1",
        })
    }
    fn binary(self) -> &'static str {
        match self.package {
            Package::Tunnel => "tunnel-client",
            Package::Adb => "adb",
        }
    }
    fn owner(self) -> &'static str {
        match self.package {
            Package::Tunnel => OWNER,
            Package::Adb => "TV_CONTROL_ADB_DEPENDENCY_V1",
        }
    }
    fn url(self) -> String {
        if self.package == Package::Adb {
            return format!("https://dl.google.com/android/repository/{}", self.asset());
        }
        format!("{ROOT}/{VERSION}/{}", self.asset())
    }
    fn payload(self) -> Vec<(String, String, bool)> {
        if self.package == Package::Adb {
            return ["adb", "NOTICE.txt", "source.properties"]
                .into_iter()
                .map(|name| (format!("platform-tools/{name}"), name.into(), name == "adb"))
                .collect();
        }
        self.members()
            .into_iter()
            .map(|name| {
                let executable = name == "tunnel-client" || name == "cloudflared";
                (name.clone(), name, executable)
            })
            .collect()
    }
    fn asset(self) -> String {
        if self.package == Package::Adb {
            return "platform-tools_r37.0.1-linux.zip".into();
        }

        format!("tunnel-client-{VERSION}-{}.zip", self.platform)
    }
    fn members(self) -> Vec<String> {
        if self.package == Package::Adb {
            return [
                "NOTICE.txt",
                "adb",
                "etc1tool",
                "fastboot",
                "hprof-conv",
                "lib64/libc++.so",
                "make_f2fs",
                "make_f2fs_casefold",
                "mke2fs",
                "mke2fs.conf",
                "source.properties",
                "sqlite3",
            ]
            .into_iter()
            .map(|name| format!("platform-tools/{name}"))
            .collect();
        }

        vec![
            "tunnel-client".into(),
            "cloudflared".into(),
            "cloudflared-manifest.json".into(),
            "LICENSE".into(),
            "NOTICE".into(),
            format!("tunnel-client-{VERSION}-{}-licenses.txt", self.platform),
            format!("tunnel-client-{VERSION}-{}.spdx.json", self.platform),
        ]
    }
    pub fn binary_path(self, state: &Path) -> PathBuf {
        self.directory(state).join(self.binary())
    }
    fn directory(self, state: &Path) -> PathBuf {
        if self.package == Package::Adb {
            return state.join("adb-37.0.1-linux-amd64");
        }

        state.join(format!("tunnel-client-{VERSION}-{}", self.platform))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    owner: String,
    archive_sha256: String,
    files: BTreeMap<String, String>,
}
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
fn private_directory(path: &Path) -> Result<(), &'static str> {
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
fn private_file(path: &Path, executable: bool) -> Result<File, &'static str> {
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
fn write_new(path: &Path, bytes: &[u8], executable: bool) -> Result<(), &'static str> {
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
fn system_program(name: &str) -> Result<PathBuf, &'static str> {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|p| Path::new(p).join(name))
        .find(|p| p.is_file())
        .ok_or("tunnel_dependency_system_tool_missing")
}
fn invoke(
    process: &dyn Process,
    program: PathBuf,
    args: Vec<OsString>,
    ctx: &Context<'_>,
) -> Result<Vec<u8>, &'static str> {
    let env = BTreeMap::from([
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("LC_ALL".into(), "C".into()),
    ]);
    let output = process
        .run(
            Invocation {
                program,
                args,
                env,
                stdin: vec![],
                mutating: false,
            },
            ctx,
        )
        .map_err(|e| e.code)?;
    if !output.success {
        return Err("tunnel_dependency_tool_failed");
    }
    Ok(output.stdout)
}
fn digest(process: &dyn Process, path: &Path, ctx: &Context<'_>) -> Result<String, &'static str> {
    let (program, args) = match system_program("sha256sum") {
        Ok(program) => (program, vec![path.as_os_str().into()]),
        Err(_) => (
            system_program("shasum")?,
            vec!["-a".into(), "256".into(), path.as_os_str().into()],
        ),
    };
    let output = invoke(process, program, args, ctx)?;
    let value = std::str::from_utf8(&output)
        .map_err(|_| "tunnel_dependency_digest_invalid")?
        .split_whitespace()
        .next()
        .ok_or("tunnel_dependency_digest_invalid")?;
    if value.len() != 64 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("tunnel_dependency_digest_invalid");
    }
    Ok(value.to_ascii_lowercase())
}
fn executable_header(bytes: &[u8], release: Release) -> Result<(), &'static str> {
    if bytes.len() < 20
        || bytes[..6] != *b"\x7fELF\x02\x01"
        || !matches!(&bytes[16..18], [2, 0] | [3, 0])
    {
        return Err("tunnel_dependency_binary_format_mismatch");
    }
    let machine = if release.platform == "linux-amd64" {
        [62, 0]
    } else {
        [183, 0]
    };
    if bytes[18..20] != machine {
        return Err("tunnel_dependency_architecture_mismatch");
    }
    Ok(())
}
/// Read-only reuse. A foreign, modified, partial, or wrong-platform installation fails closed.
pub fn installed(
    state: &Path,
    release: Release,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> Result<Option<PathBuf>, &'static str> {
    private_directory(state)?;
    let directory = release.directory(state);
    match directory.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("tunnel_dependency_directory_unreadable"),
        Ok(_) => {}
    }
    private_directory(&directory)?;
    let mut bytes = Vec::new();
    private_file(&directory.join("receipt.json"), false)?
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| "tunnel_dependency_receipt_unreadable")?;
    if bytes.len() > 8192 {
        return Err("tunnel_dependency_receipt_invalid");
    }
    let receipt: Receipt =
        serde_json::from_slice(&bytes).map_err(|_| "tunnel_dependency_receipt_invalid")?;
    let members = release.payload();
    if receipt.owner != release.owner()
        || receipt.archive_sha256 != release.sha256
        || receipt.files.len() != members.len()
    {
        return Err("tunnel_dependency_receipt_mismatch");
    }
    if fs::read_dir(&directory)
        .map_err(|_| "tunnel_dependency_directory_unreadable")?
        .count()
        != members.len() + 1
    {
        return Err("tunnel_dependency_unexpected_file");
    }
    for (_, member, executable) in members {
        let file = directory.join(&member);
        private_file(&file, executable)?;
        if receipt.files.get(&member) != Some(&digest(process, &file, ctx)?) {
            return Err("tunnel_dependency_file_modified");
        }
    }
    Ok(Some(directory.join(release.binary())))
}
/// Verify only our generated private ADB namespace. Other executable paths remain
/// explicit trusted configuration. Call before resolving symlinks or executing ADB.
pub fn verify_adb_path(
    binary: &Path,
    configuration_parent: &Path,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> Result<(), &'static str> {
    let state = configuration_parent.join("connection-dependencies");
    if !binary.starts_with(&state) {
        return Ok(());
    }
    if std::env::consts::OS != "linux" {
        return Err("adb_private_installation_platform_mismatch");
    }
    let release = Release::adb_linux(std::env::consts::ARCH)?;
    if binary != release.binary_path(&state) {
        return Err("adb_private_installation_path_invalid");
    }
    if installed(&state, release, process, ctx)?.as_deref() != Some(binary) {
        return Err("adb_private_installation_missing");
    }
    Ok(())
}
struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
/// Caller must obtain approval first. No credential, runtime, or account operations occur here.
pub fn install_linux(
    state: &Path,
    release: Release,
    approved: bool,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> Result<PathBuf, &'static str> {
    if !approved {
        return Err("tunnel_dependency_approval_required");
    }
    ctx.check().map_err(|e| e.code)?;
    private_directory(state)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(state.join("dependency.lock"))
        .map_err(|_| "tunnel_dependency_lock_failed")?;
    private_file(&state.join("dependency.lock"), false)?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("tunnel_dependency_locked");
    }
    if let Some(binary) = installed(state, release, process, ctx)? {
        return Ok(binary);
    }
    let stage = state.join(format!(
        ".dependency-{}",
        crate::connection_proof::random_token()?
    ));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&stage)
        .map_err(|_| "tunnel_dependency_stage_failed")?;
    let stage = Stage(stage);
    let archive = stage.0.join("archive.zip");
    write_new(&archive, &[], false)?;
    invoke(
        process,
        system_program("curl")?,
        [
            "-q",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "--connect-timeout",
            "10",
            "--max-time",
            "120",
            "--max-filesize",
            "33554432",
            "--output",
        ]
        .into_iter()
        .map(OsString::from)
        .chain([archive.as_os_str().into(), release.url().into()])
        .collect(),
        ctx,
    )?;
    if private_file(&archive, false)?
        .metadata()
        .map_err(|_| "tunnel_dependency_file_unreadable")?
        .len()
        > MAX_ARCHIVE
    {
        return Err("tunnel_dependency_archive_too_large");
    }
    if digest(process, &archive, ctx)? != release.sha256 {
        return Err("tunnel_dependency_checksum_mismatch");
    }
    let list = invoke(
        process,
        system_program("unzip")?,
        vec!["-Z1".into(), archive.as_os_str().into()],
        ctx,
    )?;
    let text = std::str::from_utf8(&list).map_err(|_| "tunnel_dependency_archive_invalid")?;
    let mut actual: Vec<_> = text.lines().collect();
    actual.sort_unstable();
    let mut expected = release.members();
    expected.sort_unstable();
    if actual != expected {
        return Err("tunnel_dependency_archive_members_invalid");
    }
    let payload = stage.0.join("payload");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&payload)
        .map_err(|_| "tunnel_dependency_stage_failed")?;
    let mut files = BTreeMap::new();
    for (source, member, executable) in release.payload() {
        ctx.check().map_err(|e| e.code)?;
        // Streaming a single exact member to memory never follows archive links or paths.
        let bytes = invoke(
            process,
            system_program("unzip")?,
            vec!["-p".into(), archive.as_os_str().into(), source.into()],
            ctx,
        )?;
        if bytes.len() > MAX_MEMBER {
            return Err("tunnel_dependency_member_too_large");
        }
        if executable {
            executable_header(&bytes, release)?;
        }
        let file = payload.join(&member);
        write_new(&file, &bytes, executable)?;
        files.insert(member, digest(process, &file, ctx)?);
    }
    let receipt = Receipt {
        owner: release.owner().into(),
        archive_sha256: release.sha256.into(),
        files,
    };
    write_new(
        &payload.join("receipt.json"),
        &serde_json::to_vec(&receipt).map_err(|_| "tunnel_dependency_receipt_invalid")?,
        false,
    )?;
    ctx.check().map_err(|e| e.code)?;
    let destination = release.directory(state);
    if destination.symlink_metadata().is_ok() {
        return Err("tunnel_dependency_destination_exists");
    }
    fs::rename(&payload, &destination).map_err(|_| "tunnel_dependency_publish_failed")?;
    installed(state, release, process, ctx)?.ok_or("tunnel_dependency_publish_failed")
}

/// Official supported macOS path. Caller explains that Homebrew manages its own
/// prefix and current formula version; this is not a pinned private Linux install.
pub fn install_homebrew(
    brew: &Path,
    approved: bool,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> Result<(), &'static str> {
    if !approved {
        return Err("tunnel_dependency_approval_required");
    }
    if !brew.is_absolute() {
        return Err("tunnel_dependency_brew_path_invalid");
    }
    install_homebrew_package(brew, approved, "openai/tools/tunnel-client", process, ctx)
}
pub fn install_adb_homebrew(
    brew: &Path,
    approved: bool,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> Result<(), &'static str> {
    install_homebrew_package(brew, approved, "android-platform-tools", process, ctx)
}
fn install_homebrew_package(
    brew: &Path,
    approved: bool,
    package: &str,
    process: &dyn Process,
    ctx: &Context<'_>,
) -> Result<(), &'static str> {
    if !approved {
        return Err("tunnel_dependency_approval_required");
    }
    if !brew.is_absolute() {
        return Err("tunnel_dependency_brew_path_invalid");
    }
    let mut env = BTreeMap::new();
    for key in ["HOME", "PATH", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) {
            env.insert(key.into(), value);
        }
    }
    env.insert("HOMEBREW_NO_AUTO_UPDATE".into(), "1".into());
    env.insert("HOMEBREW_NO_ANALYTICS".into(), "1".into());
    env.insert("HOMEBREW_NO_ENV_HINTS".into(), "1".into());
    let result = process
        .run(
            Invocation {
                program: brew.to_owned(),
                args: vec!["install".into(), package.into()],
                env,
                stdin: vec![],
                mutating: true,
            },
            ctx,
        )
        .map_err(|e| e.code)?;
    if !result.success {
        return Err("tunnel_dependency_homebrew_failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        process::Output,
        runtime::{CancelFlag, SystemClock},
    };
    use std::{
        os::unix::fs::symlink,
        sync::{
            atomic::{AtomicU64, Ordering},
            Mutex,
        },
    };
    use tv_application::Clock;
    use tv_domain::PortResult;
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let p = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "tv-dependency-test-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct Mock {
        release: Release,
        bad_checksum: bool,
        bad_members: bool,
        bad_machine: bool,
        calls: Mutex<Vec<Invocation>>,
    }
    impl Mock {
        fn new() -> Self {
            Self {
                release: Release::linux("x86_64").unwrap(),
                bad_checksum: false,
                bad_members: false,
                bad_machine: false,
                calls: Mutex::new(vec![]),
            }
        }
    }
    impl Process for Mock {
        fn run(&self, i: Invocation, _: &Context<'_>) -> PortResult<Output> {
            let program = i.program.file_name().unwrap().to_str().unwrap();
            let args: Vec<_> = i
                .args
                .iter()
                .map(|x| x.to_string_lossy().into_owned())
                .collect();
            let stdout = match program {
                "curl" => {
                    assert!(args.contains(&"=https".into()));
                    assert_eq!(args.last().unwrap(), &self.release.url());
                    let output = args.iter().position(|x| x == "--output").unwrap() + 1;
                    fs::write(&args[output], b"fixture zip").unwrap();
                    vec![]
                }
                "sha256sum" | "shasum" => {
                    let file = Path::new(args.last().unwrap());
                    if file.file_name().unwrap() == "archive.zip" {
                        if self.bad_checksum {
                            "0".repeat(64).into_bytes()
                        } else {
                            self.release.sha256.as_bytes().to_vec()
                        }
                    } else {
                        // Deterministic mock digest; not a production hash implementation.
                        let bytes = fs::read(file).unwrap();
                        format!("{:064x}", bytes.iter().map(|b| *b as u64).sum::<u64>())
                            .into_bytes()
                    }
                }
                "unzip" if args[0] == "-Z1" => {
                    if self.bad_members {
                        b"../tunnel-client\ntunnel-client\n".to_vec()
                    } else {
                        self.release.members().join("\n").into_bytes()
                    }
                }
                "unzip" => {
                    assert_eq!(args[0], "-p");
                    if args[2] == "tunnel-client"
                        || args[2] == "cloudflared"
                        || args[2] == "platform-tools/adb"
                    {
                        let mut bytes = vec![0; 32];
                        bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
                        bytes[16] = 2;
                        bytes[18] = if self.bad_machine { 183 } else { 62 };
                        bytes
                    } else {
                        b"fixture data".to_vec()
                    }
                }
                "brew" => {
                    assert_eq!(
                        args,
                        [
                            "install",
                            if self.release.package == Package::Adb {
                                "android-platform-tools"
                            } else {
                                "openai/tools/tunnel-client"
                            }
                        ]
                    );
                    vec![]
                }
                _ => panic!("unexpected program: {program}"),
            };
            self.calls.lock().unwrap().push(i);
            Ok(Output {
                success: true,
                stdout,
            })
        }
    }
    fn context<'a>(clock: &'a SystemClock, cancel: &'a CancelFlag) -> Context<'a> {
        Context {
            clock,
            cancellation: cancel,
            deadline_monotonic_ms: clock.monotonic_ms() + 10_000,
        }
    }
    #[test]
    #[ignore = "requires an explicitly supplied official ZIP; never executes its payload"]
    fn official_archive_offline_contract() {
        offline_archive(
            "TV_CONTROL_TEST_TUNNEL_ARCHIVE",
            Release::linux(std::env::consts::ARCH).unwrap(),
        );
    }
    #[test]
    #[ignore = "requires an explicit official Google ZIP; never executes ADB"]
    fn official_adb_archive_offline_contract() {
        offline_archive(
            "TV_CONTROL_TEST_ADB_ARCHIVE",
            Release::adb_linux(std::env::consts::ARCH).unwrap(),
        );
    }
    fn offline_archive(variable: &str, release: Release) {
        struct ArchiveFixture(PathBuf);
        impl Process for ArchiveFixture {
            fn run(&self, i: Invocation, ctx: &Context<'_>) -> PortResult<Output> {
                if i.program.ends_with("curl") {
                    let output = i.args.iter().position(|s| s == "--output").unwrap() + 1;
                    fs::write(Path::new(&i.args[output]), fs::read(&self.0).unwrap()).unwrap();
                    return Ok(Output {
                        success: true,
                        stdout: vec![],
                    });
                }
                assert!(i.program.ends_with("unzip") || i.program.ends_with("sha256sum"));
                crate::process::BoundedProcess {
                    max_output: MAX_MEMBER,
                    poll_ms: 10,
                }
                .run(i, ctx)
            }
        }
        let source =
            PathBuf::from(std::env::var_os(variable).expect("explicit fixture archive required"));
        let p = ArchiveFixture(source);
        let d = Directory::new();
        let clock = SystemClock::default();
        let cancel = CancelFlag::default();
        let ctx = Context {
            clock: &clock,
            cancellation: &cancel,
            deadline_monotonic_ms: clock.monotonic_ms() + 60_000,
        };
        let binary = install_linux(&d.0, release, true, &p, &ctx).unwrap();
        assert_eq!(installed(&d.0, release, &p, &ctx).unwrap(), Some(binary));
    }
    #[test]
    fn selection_is_explicit_and_pinned() {
        assert_eq!(
            Release::linux("x86_64").unwrap().asset(),
            "tunnel-client-v0.0.17-linux-amd64.zip"
        );
        assert_eq!(Release::linux("aarch64").unwrap().platform, "linux-arm64");
        assert!(Release::linux("armv7").is_err());
        let adb = Release::adb_linux("x86_64").unwrap();
        assert!(adb
            .url()
            .starts_with("https://dl.google.com/android/repository/"));
        assert_eq!(adb.members().len(), 12);
        assert_eq!(adb.payload().len(), 3);
        assert_eq!(
            adb.payload()[0],
            ("platform-tools/adb".into(), "adb".into(), true)
        );
        assert!(Release::adb_linux("aarch64").is_err());
    }
    #[test]
    fn adb_installs_only_required_executable_and_notices_and_resumes() {
        let d = Directory::new();
        let mut p = Mock::new();
        p.release = Release::adb_linux("x86_64").unwrap();
        let clock = SystemClock::default();
        let cancel = CancelFlag::default();
        let ctx = context(&clock, &cancel);
        let root = prepare_directory(&d.0, true).unwrap();
        let binary = install_linux(&root, p.release, true, &p, &ctx).unwrap();
        assert_eq!(binary.file_name().unwrap(), "adb");
        assert!(binary.parent().unwrap().join("NOTICE.txt").is_file());
        assert!(!binary.parent().unwrap().join("fastboot").exists());
        assert_eq!(
            installed(&root, p.release, &p, &ctx).unwrap(),
            Some(binary.clone())
        );
        if std::env::consts::OS == "linux" && std::env::consts::ARCH == "x86_64" {
            verify_adb_path(&binary, &d.0, &p, &ctx).unwrap();
            fs::write(&binary, b"tampered").unwrap();
            assert_eq!(
                verify_adb_path(&binary, &d.0, &p, &ctx).unwrap_err(),
                "tunnel_dependency_file_modified"
            );
        } else {
            assert!(verify_adb_path(&binary, &d.0, &p, &ctx).is_err());
        }
        install_adb_homebrew(Path::new("/opt/homebrew/bin/brew"), true, &p, &ctx).unwrap();
        assert!(!p
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|i| i.program.ends_with("adb")));
    }
    #[test]
    fn private_root_requires_consent_and_refuses_foreign_paths() {
        let d = Directory::new();
        assert!(prepare_directory(&d.0, false).is_err());
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 0);
        symlink(&d.0, d.0.join("connection-dependencies")).unwrap();
        assert!(prepare_directory(&d.0, true).is_err());
    }
    #[test]
    fn no_consent_no_process_or_changes() {
        let d = Directory::new();
        let p = Mock::new();
        let clock = SystemClock::default();
        let cancel = CancelFlag::default();
        let ctx = context(&clock, &cancel);
        assert_eq!(
            install_linux(&d.0, Release::linux("x86_64").unwrap(), false, &p, &ctx).unwrap_err(),
            "tunnel_dependency_approval_required"
        );
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 0);
        assert!(install_homebrew(Path::new("/opt/homebrew/bin/brew"), false, &p, &ctx).is_err());
        assert!(p.calls.lock().unwrap().is_empty());
    }
    #[test]
    fn expiration_and_competing_install_do_not_download() {
        let d = Directory::new();
        let p = Mock::new();
        let clock = SystemClock::default();
        let cancel = CancelFlag::default();
        let expired = Context {
            clock: &clock,
            cancellation: &cancel,
            deadline_monotonic_ms: 0,
        };
        let release = Release::linux("x86_64").unwrap();
        assert!(install_linux(&d.0, release, true, &p, &expired).is_err());
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 0);
        write_new(&d.0.join("dependency.lock"), &[], false).unwrap();
        let lock = private_file(&d.0.join("dependency.lock"), false).unwrap();
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert_eq!(
            install_linux(&d.0, release, true, &p, &context(&clock, &cancel)).unwrap_err(),
            "tunnel_dependency_locked"
        );
        assert!(p.calls.lock().unwrap().is_empty());
    }
    #[test]
    fn install_resume_and_tamper_detection() {
        let d = Directory::new();
        let p = Mock::new();
        let clock = SystemClock::default();
        let cancel = CancelFlag::default();
        let ctx = context(&clock, &cancel);
        let release = Release::linux("x86_64").unwrap();
        let binary = install_linux(&d.0, release, true, &p, &ctx).unwrap();
        assert_eq!(binary.metadata().unwrap().mode() & 0o777, 0o700);
        assert_eq!(
            install_linux(&d.0, release, true, &p, &ctx).unwrap(),
            binary
        );
        assert_eq!(
            p.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|i| i.program.ends_with("curl"))
                .count(),
            1
        );
        assert!(!p
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|i| i.program.ends_with("tunnel-client")));
        fs::write(&binary, b"changed").unwrap();
        assert_eq!(
            installed(&d.0, release, &p, &ctx).unwrap_err(),
            "tunnel_dependency_file_modified"
        );
    }
    #[test]
    fn rejects_untrusted_archives_and_cleans_stage() {
        for kind in 0..3 {
            let d = Directory::new();
            let mut p = Mock::new();
            p.bad_checksum = kind == 0;
            p.bad_members = kind == 1;
            p.bad_machine = kind == 2;
            let clock = SystemClock::default();
            let cancel = CancelFlag::default();
            let ctx = context(&clock, &cancel);
            let release = Release::linux("x86_64").unwrap();
            let error = install_linux(&d.0, release, true, &p, &ctx).unwrap_err();
            assert_eq!(
                error,
                [
                    "tunnel_dependency_checksum_mismatch",
                    "tunnel_dependency_archive_members_invalid",
                    "tunnel_dependency_architecture_mismatch"
                ][kind]
            );
            assert!(!release.directory(&d.0).exists());
            assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1); // lock only
        }
    }
    #[test]
    fn refuses_foreign_and_symlink_destinations() {
        let d = Directory::new();
        let p = Mock::new();
        let clock = SystemClock::default();
        let cancel = CancelFlag::default();
        let ctx = context(&clock, &cancel);
        let release = Release::linux("x86_64").unwrap();
        let target = release.directory(&d.0);
        symlink(&d.0, &target).unwrap();
        assert_eq!(
            install_linux(&d.0, release, true, &p, &ctx).unwrap_err(),
            "tunnel_dependency_symlink_refused"
        );
        fs::remove_file(&target).unwrap();
        fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
        fs::write(target.join("foreign"), b"preserve").unwrap();
        assert!(install_linux(&d.0, release, true, &p, &ctx).is_err());
        assert_eq!(fs::read(target.join("foreign")).unwrap(), b"preserve");
        assert!(p.calls.lock().unwrap().is_empty());
    }
    #[test]
    fn homebrew_has_distinct_consent_and_no_tunnel_credentials() {
        let p = Mock::new();
        let clock = SystemClock::default();
        let cancel = CancelFlag::default();
        let ctx = context(&clock, &cancel);
        install_homebrew(Path::new("/opt/homebrew/bin/brew"), true, &p, &ctx).unwrap();
        let calls = p.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].mutating);
        assert!(!calls[0]
            .env
            .contains_key(&OsString::from("TV_CONTROL_TUNNEL_KEY")));
    }
}
