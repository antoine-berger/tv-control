use super::release::Package;
use super::*;
use crate::{
    process::Output,
    runtime::{CancelFlag, SystemClock},
};
use std::{
    os::unix::fs::{symlink, MetadataExt},
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
                    format!("{:064x}", bytes.iter().map(|b| *b as u64).sum::<u64>()).into_bytes()
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
        let alias = d.0.join("parent-alias");
        symlink(&d.0, &alias).unwrap();
        let alias_binary = alias.join(binary.strip_prefix(&d.0).unwrap());
        verify_adb_path(&alias_binary, &d.0, &p, &ctx).unwrap();
        fs::write(&binary, b"tampered").unwrap();
        assert_eq!(
            verify_adb_path(&alias_binary, &d.0, &p, &ctx).unwrap_err(),
            "tunnel_dependency_file_modified"
        );
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

#[test]
fn dependency_lock_releases_at_owner_end_while_duplicate_remains_open() {
    let d = Directory::new();
    let path = d.0.join("dependency.lock");
    write_new(&path, &[], false).unwrap();
    let owner = DependencyLock::acquire(private_file(&path, false).unwrap()).unwrap();
    let duplicate = owner.0.try_clone().unwrap();
    // A failed acquisition owns only its newly opened File, never this lock.
    for _ in 0..2 {
        assert!(matches!(
            DependencyLock::acquire(private_file(&path, false).unwrap()),
            Err("tunnel_dependency_locked")
        ));
    }
    drop(owner);
    let next = DependencyLock::acquire(private_file(&path, false).unwrap()).unwrap();
    assert_eq!(
        duplicate.metadata().unwrap().ino(),
        next.0.metadata().unwrap().ino()
    );
    drop(next);
    drop(duplicate);
}
#[test]
fn dependency_lock_releases_on_success_and_early_error_with_live_duplicate() {
    for fail_early in [false, true] {
        let d = Directory::new();
        let path = d.0.join("dependency.lock");
        write_new(&path, &[], false).unwrap();
        let mut duplicate = None;
        let result = (|| -> Result<(), &'static str> {
            let owner = DependencyLock::acquire(private_file(&path, false).unwrap())?;
            duplicate = Some(owner.0.try_clone().unwrap());
            if fail_early {
                return Err("fixture_early_error");
            }
            Ok(())
        })();
        assert_eq!(
            result,
            if fail_early {
                Err("fixture_early_error")
            } else {
                Ok(())
            }
        );
        let next = DependencyLock::acquire(private_file(&path, false).unwrap()).unwrap();
        assert_eq!(
            duplicate.as_ref().unwrap().metadata().unwrap().ino(),
            next.0.metadata().unwrap().ino()
        );
        drop(next);
        drop(duplicate);
    }
}
