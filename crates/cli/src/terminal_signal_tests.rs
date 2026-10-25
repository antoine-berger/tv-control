//! Process-local signal dispositions must never be changed in parallel test workers.
use super::*;
use std::{
    process::{Child, Command},
    sync::atomic::AtomicUsize,
    time::{Duration, Instant},
};

const SIGNALS: [i32; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];
static PREVIOUS_CALLED: AtomicUsize = AtomicUsize::new(0);
extern "C" fn previous(signal: i32) {
    PREVIOUS_CALLED.fetch_or(1 << signal, Ordering::Relaxed);
}

struct Restore([(i32, libc::sigaction); 3]);
impl Drop for Restore {
    fn drop(&mut self) {
        for (signal, action) in self.0.iter().rev() {
            assert_eq!(
                unsafe { libc::sigaction(*signal, action, std::ptr::null_mut()) },
                0
            );
        }
    }
}
fn snapshot(signal: i32) -> libc::sigaction {
    let mut action = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::sigaction(signal, std::ptr::null(), &mut action) },
        0
    );
    action
}
fn assert_restored(expected: &[(i32, libc::sigaction); 3]) {
    for (signal, before) in expected {
        let after = snapshot(*signal);
        assert_eq!(after.sa_sigaction, before.sa_sigaction);
        assert_eq!(after.sa_flags, before.sa_flags);
        for member in [libc::SIGUSR1, libc::SIGUSR2, *signal] {
            assert_eq!(
                unsafe { libc::sigismember(&after.sa_mask, member) },
                unsafe { libc::sigismember(&before.sa_mask, member) }
            );
        }
        INTERRUPTED.store(false, Ordering::Relaxed);
        PREVIOUS_CALLED.store(0, Ordering::Relaxed);
        assert_eq!(unsafe { libc::raise(*signal) }, 0);
        assert_eq!(PREVIOUS_CALLED.load(Ordering::Relaxed), 1 << signal);
        assert!(!INTERRUPTED.load(Ordering::Relaxed));
    }
}

#[test]
fn isolated_signal_contract() {
    let Ok(mode) = std::env::var("TV_CONTROL_SIGNAL_TEST_CHILD") else {
        return;
    };
    let _restore = Restore(SIGNALS.map(|signal| (signal, snapshot(signal))));
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = previous as *const () as usize;
    action.sa_flags = libc::SA_RESTART;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaddset(&mut action.sa_mask, libc::SIGUSR1);
    }
    for signal in SIGNALS {
        assert_eq!(
            unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) },
            0
        );
    }
    let expected = SIGNALS.map(|signal| (signal, snapshot(signal)));
    match mode.as_str() {
        "restore" => {
            let interrupts = Interrupts::install().unwrap();
            for signal in SIGNALS {
                INTERRUPTED.store(false, Ordering::Relaxed);
                PREVIOUS_CALLED.store(0, Ordering::Relaxed);
                assert_eq!(unsafe { libc::raise(signal) }, 0);
                assert!(interrupts.is_cancelled());
                assert_eq!(PREVIOUS_CALLED.load(Ordering::Relaxed), 0);
            }
            drop(interrupts);
            assert_restored(&expected);
            // Both an inherited ignored and default HUP disposition must be
            // replaced while the guard is live and restored afterward.
            for handler in [libc::SIG_IGN, libc::SIG_DFL] {
                let mut hup: libc::sigaction = unsafe { std::mem::zeroed() };
                hup.sa_sigaction = handler;
                unsafe {
                    libc::sigemptyset(&mut hup.sa_mask);
                    assert_eq!(libc::sigaction(libc::SIGHUP, &hup, std::ptr::null_mut()), 0);
                }
                let before = snapshot(libc::SIGHUP);
                let interrupts = Interrupts::install().unwrap();
                assert_eq!(unsafe { libc::raise(libc::SIGHUP) }, 0);
                assert!(interrupts.is_cancelled());
                drop(interrupts);
                let after = snapshot(libc::SIGHUP);
                assert_eq!(after.sa_sigaction, before.sa_sigaction);
                assert_eq!(after.sa_flags, before.sa_flags);
            }
        }
        "rollback" => {
            for index in 0..SIGNALS.len() {
                let mut signals = SIGNALS;
                signals[index] = libc::SIGKILL; // Cannot have a user handler.
                assert!(matches!(
                    Interrupts::install_signals(signals),
                    Err("signal_setup_failed")
                ));
                assert_restored(&expected);
            }
        }
        _ => panic!("unknown fixture mode"),
    }
}

struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
#[test]
fn signal_handlers_restore_and_rollback_in_owned_isolated_children() {
    for mode in ["restore", "rollback"] {
        let mut child = Owned(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "terminal::signal_tests::isolated_signal_contract",
                    "--nocapture",
                ])
                .env("TV_CONTROL_SIGNAL_TEST_CHILD", mode)
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success(), "signal fixture {mode} failed");
                break;
            }
            assert!(Instant::now() < deadline, "signal fixture {mode} timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
