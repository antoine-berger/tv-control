use super::*;
fn denied(_: libc::pid_t) -> io::Result<()> {
    Err(io::Error::from_raw_os_error(libc::EPERM))
}

#[test]
fn singleton_retained_zombie_proves_injected_darwin_eperm_is_benign() {
    let mut group = spawn("exit 0");
    assert!(exited(&mut group));
    group
        .cleanup_with_members(denied, |pid, ids| {
            ids[0] = pid;
            1
        })
        .unwrap();
    assert!(!group.owned);
}

#[test]
fn incomplete_or_other_member_keeps_original_permission_error() {
    for count in [-1, 0, 2, 3] {
        let mut group = spawn("exit 0");
        assert!(exited(&mut group));
        let error = group
            .cleanup_with_members(denied, |pid, ids| {
                ids[0] = pid;
                count
            })
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EPERM));
        assert!(group.owned);
        group.cleanup().unwrap();
    }
    let mut group = spawn("exit 0");
    assert!(exited(&mut group));
    let error = group
        .cleanup_with_members(denied, |_, ids| {
            ids[0] = 1;
            1
        })
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    group.cleanup().unwrap();
}

#[test]
fn live_leader_never_uses_singleton_permission_exception() {
    let mut group = spawn("exec sleep 30");
    let error = group
        .cleanup_with_members(denied, |_, _| panic!("live leader enumerated"))
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    group.cleanup().unwrap();
}

#[test]
fn ownership_lost_during_singleton_proof_keeps_eperm_and_disarms() {
    let mut group = spawn("exit 0");
    assert!(exited(&mut group));
    let error = group
        .cleanup_with_members(denied, |pid, ids| {
            ids[0] = pid;
            // Reap only this exited fixture to simulate an unexpected competing reaper.
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
            1
        })
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    assert!(!group.owned);
    group
        .cleanup_with(|_| panic!("signal after lost ownership"))
        .unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn native_darwin_successful_child_cleanup_keeps_zombie_until_proven_safe() {
    let mut group = spawn("exit 0");
    assert!(exited(&mut group));
    assert!(group.singleton_zombie(&mut group_members));
    group.cleanup().unwrap();
    assert!(!group.owned);
    assert!(group.child.wait().unwrap().success());
}

#[test]
fn ownership_lost_before_singleton_proof_does_not_enumerate_or_signal_again() {
    let mut group = spawn("exit 0");
    assert!(exited(&mut group));
    let pid = group.child.id() as libc::pid_t;
    let error = group
        .cleanup_with_members(
            |target| {
                assert_eq!(target, -pid);
                let mut status = 0;
                assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
                denied(target)
            },
            |_, _| panic!("enumerated after ownership loss"),
        )
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    assert!(!group.owned);
    group
        .cleanup_with(|_| panic!("signal after ownership loss"))
        .unwrap();
}

#[test]
fn exit_during_group_signal_skips_later_positive_pid_signal() {
    let mut group = spawn("exec sleep 30");
    let pid = group.child.id() as libc::pid_t;
    assert_eq!(group.poll_exit().unwrap(), None);
    let mut signals = 0;
    group
        .cleanup_with_members(
            |target| {
                signals += 1;
                assert_eq!(signals, 1, "unnecessary signal after fresh exit proof");
                assert_eq!(target, -pid);
                // Terminate only our fixture, then observe (never reap) its exit.
                assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                    assert_eq!(
                        unsafe {
                            libc::waitid(
                                libc::P_PID,
                                pid as libc::id_t,
                                &mut info,
                                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                            )
                        },
                        0
                    );
                    if unsafe { info.si_pid() } == pid {
                        break;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                denied(target)
            },
            |leader, ids| {
                ids[0] = leader;
                1
            },
        )
        .unwrap();
    assert!(!group.owned);
    assert_eq!(signals, 1);
}
