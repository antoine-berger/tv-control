//! Guided prerequisite recovery. Acquiring ADB never invokes it or contacts a TV.
use super::presentation::prompt;
use super::Options;
use std::path::{Path, PathBuf};
use tv_adapters::{
    config::Config,
    runtime::{CancelFlag, SystemClock},
};
use tv_adapters::{dependency_acquisition as dependency, process::BoundedProcess};
use tv_application::Context;

const SDK_TERMS: &str = "https://developer.android.com/studio/terms";
fn verify(binary: &Path, configuration_parent: &Path) -> Result<(), &'static str> {
    let clock = SystemClock::default();
    let cancel = CancelFlag::default();
    let ctx = Context {
        clock: &clock,
        cancellation: &cancel,
        deadline_monotonic_ms: 10_000,
    };
    let process = BoundedProcess {
        max_output: 65_536,
        poll_ms: 10,
    };
    dependency::verify_adb_path(binary, configuration_parent, &process, &ctx)
}
pub(super) fn verify_configuration(config: &Config, output: &Path) -> Result<(), &'static str> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .map_err(|_| "output_directory_missing")?;
    verify(&config.executables.adb, &parent)
}
fn existing(options: &Options) -> Result<PathBuf, &'static str> {
    // Verify before canonicalization: a changed private symlink must not become
    // an apparently external trusted executable.
    if let Some(path) = options.get("--adb") {
        verify(Path::new(path), &parent(options)?)?;
    }
    // An explicit path always wins; an invalid one never silently falls back.
    let binary = executable(options, "--adb", "adb").or_else(|error| {
        if options.get("--adb").is_some() || std::env::consts::OS != "macos" {
            return Err(error);
        }
        for path in ["/opt/homebrew/bin/adb", "/usr/local/bin/adb"] {
            let mut explicit = options.clone();
            explicit.values.insert("--adb".into(), path.into());
            if let Ok(binary) = executable(&explicit, "--adb", "adb") {
                return Ok(binary);
            }
        }
        Err(error)
    })?;
    verify(&binary, &parent(options)?)?;
    Ok(binary)
}
fn parent(options: &Options) -> Result<PathBuf, &'static str> {
    Path::new(options.get("--output").unwrap_or("tv-control.local.json"))
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .map_err(|_| "output_directory_missing")
}
fn brew() -> Option<PathBuf> {
    executable(&Options::default(), "--brew", "brew")
        .ok()
        .or_else(|| {
            ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
                .into_iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
        })
}
pub(super) fn adb(options: &Options) -> Result<PathBuf, &'static str> {
    match existing(options) {
        Ok(binary) => return Ok(binary),
        Err("adb_missing_install_official_platform_tools") => {}
        Err(error) => return Err(error),
    }
    if !options.guided || options.json || options.dry {
        return Err("adb_missing_install_official_platform_tools");
    }
    let mut selection = options.clone();
    let linux_release =
        if std::env::consts::OS == "linux" && Path::new("/lib64/ld-linux-x86-64.so.2").is_file() {
            dependency::Release::adb_linux(std::env::consts::ARCH).ok()
        } else {
            None
        };
    let brew = if std::env::consts::OS == "macos" {
        brew()
    } else {
        None
    };
    // Reuse a previously approved private download, without changing the explicit selection.
    if selection.get("--adb").is_none() {
        if let Some(release) = linux_release {
            let state = parent(options)?.join("connection-dependencies");
            if state.symlink_metadata().is_ok() {
                let clock = SystemClock::default();
                let cancel = CancelFlag::default();
                let ctx = Context {
                    clock: &clock,
                    cancellation: &cancel,
                    deadline_monotonic_ms: 10_000,
                };
                let process = BoundedProcess {
                    max_output: 65_536,
                    poll_ms: 10,
                };
                if let Some(binary) = dependency::installed(&state, release, &process, &ctx)? {
                    return Ok(binary);
                }
            }
        }
    }
    loop {
        eprintln!("ADB is missing or its selected path is unavailable.");
        if linux_release.is_some() {
            eprintln!("Install Google's verified Platform-Tools 37.0.1 ADB privately. SDK license: {SDK_TERMS}");
        } else if brew.is_some() {
            eprintln!("Install Google's current Android Platform-Tools with Homebrew in its managed prefix. SDK license: {SDK_TERMS}");
        } else {
            eprintln!("Install maintained ADB using your system package manager, then retry or enter its absolute path. Automatic installation requires glibc Linux x86-64 or macOS with Homebrew.");
        }
        let answer = prompt(if linux_release.is_some() || brew.is_some() {
            "Type install to accept the SDK license and install, retry, an absolute ADB path, or q:"
        } else {
            "Type retry, an absolute ADB path, or q:"
        })?;
        if answer == "install" && (linux_release.is_some() || brew.is_some()) {
            eprintln!("You accepted the Android SDK license: {SDK_TERMS}");
            let result = (|| {
                let clock = SystemClock::default();
                let interrupts = crate::terminal::Interrupts::install()?;
                let ctx = Context {
                    clock: &clock,
                    cancellation: &interrupts,
                    deadline_monotonic_ms: 600_000,
                };
                let process = BoundedProcess {
                    max_output: 64 * 1024 * 1024,
                    poll_ms: 20,
                };
                eprintln!("Installing ADB…");
                if let Some(release) = linux_release {
                    let state = dependency::prepare_directory(&parent(options)?, true)?;
                    dependency::install_linux(&state, release, true, &process, &ctx)
                } else {
                    dependency::install_adb_homebrew(brew.as_ref().unwrap(), true, &process, &ctx)?;
                    // The user's explicit choice to install authorizes choosing its new managed path.
                    let mut managed = selection.clone();
                    managed.values.remove("--adb");
                    existing(&managed)
                }
            })();
            match result {
                Ok(binary) => { eprintln!("ADB installed."); return Ok(binary); }
                Err("cancelled" | "expired") => return Err("setup_cancelled"),
                Err(error) => eprintln!("ADB installation paused ({error}). Correct the prerequisite and retry; saved choices are preserved."),
            }
        } else if answer == "retry" {
            if let Ok(binary) = existing(&selection) {
                return Ok(binary);
            }
        } else if Path::new(&answer).is_absolute() {
            selection.values.insert("--adb".into(), answer);
            if let Ok(binary) = existing(&selection) {
                return Ok(binary);
            }
        } else {
            eprintln!("Choose one of the displayed options.");
        }
    }
}

pub(super) fn executable(
    options: &Options,
    key: &str,
    name: &str,
) -> Result<PathBuf, &'static str> {
    use std::os::unix::fs::PermissionsExt;
    let supplied = options.get(key).map(PathBuf::from);
    let candidates = if let Some(path) = supplied {
        vec![path]
    } else {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .filter(|p| p.is_absolute())
            .map(|p| p.join(name))
            .collect()
    };
    for path in candidates {
        if let Ok(meta) = path.metadata() {
            if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                return path.canonicalize().map_err(|_| "executable_unreadable");
            }
        }
    }
    Err(if name == "adb" {
        "adb_missing_install_official_platform_tools"
    } else {
        "curl_missing_install_system_package"
    })
}
