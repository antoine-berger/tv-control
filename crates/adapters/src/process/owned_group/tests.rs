use super::*;
use std::{
    io::Read,
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn spawn(script: &str) -> OwnedGroup {
    OwnedGroup::new(
        Command::new("/bin/sh")
            .args(["-c", script])
            .process_group(0)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}
fn exited(group: &mut OwnedGroup) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(success) = group.poll_exit().unwrap() {
            return success;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn exit_observation_retains_leader_until_group_cleanup_and_reap() {
    let mut group = spawn("exit 7");
    assert!(!exited(&mut group));
    // Repeated waitid still sees the zombie: try_wait would have released it.
    assert_eq!(group.poll_exit().unwrap(), Some(false));
    group.cleanup().unwrap();
    assert!(!group.owned);
    group.cleanup_with(|_| panic!("signal after reap")).unwrap();
}

#[test]
fn lost_child_ownership_never_signals_numeric_ids() {
    let mut group = spawn("exit 0");
    assert!(exited(&mut group));
    group.child.wait().unwrap(); // Deliberately simulate an external reaper.
    let error = group
        .cleanup_with(|_| panic!("signal after ownership loss"))
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
    assert!(!group.owned);
    group
        .cleanup_with(|_| panic!("second signal after ownership loss"))
        .unwrap();
}

#[test]
fn permission_error_is_not_masked_as_zombie_only_group() {
    let mut group = spawn("exit 0");
    assert!(exited(&mut group));
    let error = group
        .cleanup_with_members(|_| Err(io::Error::from_raw_os_error(libc::EPERM)), |_, _| 0)
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    assert!(group.owned);
    group.cleanup().unwrap();
}

#[test]
fn darwin_zombie_group_esrch_is_accepted_but_live_direct_pid_esrch_is_not() {
    let mut group = spawn("exit 0");
    assert!(exited(&mut group));
    group
        .cleanup_with(|_| Err(io::Error::from_raw_os_error(libc::ESRCH)))
        .unwrap();
    assert!(!group.owned);
    assert_eq!(
        group_signal_result(io::Error::from_raw_os_error(libc::ESRCH), false)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[test]
fn cleanup_kills_descendant_holding_pipe_after_leader_exit() {
    let mut group = spawn("sleep 30 & exit 0");
    let mut stdout = group.child.stdout.take().unwrap();
    super::super::nonblocking(stdout.as_raw_fd()).unwrap();
    assert!(exited(&mut group));
    assert_eq!(
        stdout.read(&mut [0u8; 1]).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    group.cleanup().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match stdout.read(&mut [0u8; 1]) {
            Ok(0) => break,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            other => panic!("unexpected pipe state: {other:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "descendant leaked its output pipe"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn ownership_lost_between_signals_stops_cleanup_without_second_signal() {
    let mut group = spawn("exec sleep 30");
    let pid = group.child.id() as libc::pid_t;
    let mut signals = 0;
    let error = group
        .cleanup_with(|target| {
            signals += 1;
            assert_eq!(signals, 1, "second signal after external reap");
            assert_eq!(target, -pid);
            // Only signal/reap this fixture's own child, simulating a competing reaper.
            assert_eq!(unsafe { libc::kill(target, libc::SIGKILL) }, 0);
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
    assert!(!group.owned);
}

#[test]
fn old_try_wait_polling_releases_reservation_before_drop() {
    let mut group = spawn("exit 0");
    assert!(exited(&mut group));
    assert!(group.child.try_wait().unwrap().is_some());
    // Reproduce the old invariant violation without ever signaling the stale ID.
    assert_eq!(
        group.poll_exit().unwrap_err().raw_os_error(),
        Some(libc::ECHILD)
    );
    assert!(!group.owned);
}

#[test]
fn live_leader_is_terminated_and_reaped_before_cleanup_returns() {
    let mut group = spawn("exec sleep 30");
    assert_eq!(group.poll_exit().unwrap(), None);
    group.cleanup().unwrap();
    assert!(!group.owned);
    assert!(!group.child.wait().unwrap().success());
    group
        .cleanup_with(|_| panic!("signal after live leader cleanup"))
        .unwrap();
}

#[test]
fn child_leaving_initial_group_is_killed_without_signaling_shared_group() {
    use std::io::{BufRead, BufReader};
    // Use a second owned fixture group, never the test runner's shared group.
    let mut destination = spawn("exec sleep 30");
    let parent_group = destination.child.id();
    let script = format!(
        "import os,time; os.setpgid(0,{parent_group}); print('moved',flush=True); time.sleep(30)"
    );
    let mut group = OwnedGroup::new(
        Command::new("/usr/bin/python3")
            .args(["-c", &script])
            .process_group(0)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut ready = String::new();
    BufReader::new(group.child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready, "moved\n");
    assert_eq!(group.poll_exit().unwrap(), None);
    group.cleanup().unwrap();
    assert!(!group.owned);
    assert!(!group.child.wait().unwrap().success());
    assert_eq!(destination.poll_exit().unwrap(), None);
    destination.cleanup().unwrap();
}

mod darwin;
