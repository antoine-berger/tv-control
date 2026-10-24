//! A process-group ID is owned only while its child leader remains unreaped.
use std::{io, process::Child};

pub(super) struct OwnedGroup {
    pub(super) child: Child,
    owned: bool,
}
impl OwnedGroup {
    pub(super) fn new(child: Child) -> Self {
        Self { child, owned: true }
    }

    /// Observe exit without releasing the PID/PGID reservation, including while
    /// descendants still hold our pipes. No other code may reap this child.
    pub(super) fn poll_exit(&mut self) -> io::Result<Option<bool>> {
        if !self.owned {
            return Err(io::Error::from_raw_os_error(libc::ECHILD));
        }
        // SAFETY: libc supplies the platform-specific siginfo_t ABI. waitid
        // initializes the zeroed output and WNOWAIT leaves the child unreaped.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        loop {
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id() as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result == 0 {
                return Ok(if unsafe { info.si_pid() } == 0 {
                    None
                } else {
                    Some(info.si_code == libc::CLD_EXITED && unsafe { info.si_status() } == 0)
                });
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.raw_os_error() == Some(libc::ECHILD) {
                self.owned = false;
            }
            return Err(error);
        }
    }

    pub(super) fn cleanup(&mut self) -> io::Result<()> {
        self.cleanup_with(|pid| {
            // SAFETY: cleanup_with only supplies IDs reserved by our unreaped child.
            if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        })
    }

    fn cleanup_with(
        &mut self,
        signal: impl FnMut(libc::pid_t) -> io::Result<()>,
    ) -> io::Result<()> {
        self.cleanup_with_members(signal, group_members)
    }

    fn cleanup_with_members(
        &mut self,
        mut signal: impl FnMut(libc::pid_t) -> io::Result<()>,
        mut members: impl FnMut(libc::pid_t, &mut [libc::pid_t; 2]) -> i32,
    ) -> io::Result<()> {
        if !self.owned {
            return Ok(());
        }
        let exited = self.poll_exit()?.is_some();
        // SAFETY: process_group(0) created this group and waitid verified our
        // unreaped child still reserves its ID. Never signal after wait/reap.
        if let Err(error) = signal(-(self.child.id() as libc::pid_t)) {
            // Darwin may omit a zombie-only group; a live child may also have
            // moved out of its initial group. ESRCH means no group remains to
            // signal, but we must still terminate the retained direct child.
            if error.raw_os_error() != Some(libc::ESRCH)
                && !(error.raw_os_error() == Some(libc::EPERM)
                    && self.singleton_zombie(&mut members))
            {
                return Err(error);
            }
        }
        // A child can leave its initial group. Signal the still-reserved PID,
        // not Child::kill (whose implementation may itself inspect/reap exit).
        // Use the fresh observation: the leader may have exited during group
        // signaling (including the Darwin singleton proof), so no PID kill is due.
        if !exited && self.poll_exit()?.is_none() {
            if let Err(error) = signal(self.child.id() as libc::pid_t) {
                group_signal_result(error, self.poll_exit()?.is_some())?;
            }
        }
        // Disarm BEFORE the sole reaping operation, even if wait returns an error.
        self.owned = false;
        self.child.wait().map(|_| ())
    }

    fn singleton_zombie(
        &mut self,
        members: &mut impl FnMut(libc::pid_t, &mut [libc::pid_t; 2]) -> i32,
    ) -> bool {
        if !matches!(self.poll_exit(), Ok(Some(_))) {
            return false;
        }
        let pid = self.child.id() as libc::pid_t;
        let mut ids = [0; 2];
        // A full two-entry buffer cannot prove completeness. Exactly our retained
        // zombie is conclusive; no per-PID scan that could miss a concurrent fork.
        members(pid, &mut ids) == 1 && ids[0] == pid && matches!(self.poll_exit(), Ok(Some(_)))
    }
}

fn group_signal_result(error: io::Error, exited: bool) -> io::Result<()> {
    // A missing direct PID is benign only after observing its retained exit.
    // Never suppress permission errors or other evidence of incomplete cleanup.
    if exited && error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}
impl Drop for OwnedGroup {
    fn drop(&mut self) {
        // Unwinding fallback only. Normal paths explicitly surface cleanup errors.
        let _ = self.cleanup();
    }
}

#[cfg(test)]
mod tests;

// Darwin's UNIX03 killpg path filters zombies and can return EPERM, not ESRCH.
// Its proc_listpids implementation enumerates allproc + zombproc under the
// process-list lock. proc_listpgrppids selects PROC_PGRP_ONLY and returns a PID
// COUNT (not bytes). Only a complete singleton proves our known zombie is alone.
// Sources: apple-oss-distributions/xnu bsd/kern/{kern_sig,proc_info}.c and
// libsyscall/wrappers/libproc/libproc.c. Ownership requires no concurrent reaper;
// unrelated processes deliberately joining this invocation's group are outside
// that ownership contract. We never signal any ID obtained from enumeration.
#[cfg(target_os = "macos")]
fn group_members(pid: libc::pid_t, ids: &mut [libc::pid_t; 2]) -> i32 {
    // SAFETY: libc defines the ABI; the initialized array is writable for exactly
    // the supplied byte length. No hand-declared kernel structure is involved.
    unsafe {
        libc::proc_listpgrppids(
            pid,
            ids.as_mut_ptr().cast(),
            std::mem::size_of_val(ids) as libc::c_int,
        )
    }
}

#[cfg(not(target_os = "macos"))]
fn group_members(_: libc::pid_t, _: &mut [libc::pid_t; 2]) -> i32 {
    0 // No EPERM exception on platforms without the Darwin singleton proof.
}
