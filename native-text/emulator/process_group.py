"""Owned group cleanup; an unreaped direct child protects PID/PGID identity."""
import errno
import os
import signal
import time
from darwin_group import singleton_group


def require_nonreaping_wait():
    if not all(hasattr(os, name) for name in ("waitid", "P_PID", "WEXITED", "WNOHANG", "WNOWAIT")):
        raise RuntimeError("non-reaping child observation is unavailable")


def _exit_observation(process):
    require_nonreaping_wait()
    # Popen.poll()/wait() would reap the leader and release its PID for reuse.
    if process.returncode is not None:
        raise RuntimeError("owned emulator leader already reaped; refusing group signals")
    try:
        info = os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
    except ChildProcessError as error:
        raise RuntimeError("owned emulator leader ownership lost; refusing group signals") from error
    return info


def leader_exited(process):
    return _exit_observation(process) is not None


def retained_exit_info(process):
    """Read fixed exit metadata without reaping or exposing child output."""
    info = _exit_observation(process)
    if info is None:
        return {"state": "running"}
    if info.si_code == os.CLD_EXITED:
        return {"state": "exited", "exit_code": info.si_status}
    if info.si_code in (os.CLD_KILLED, os.CLD_DUMPED):
        return {"state": "signaled", "signal": info.si_status}
    return {"state": "unknown_exit"}


def signal_owned_group(process, sig):
    leader_exited(process)
    try:
        os.killpg(process.pid, sig)
    except ProcessLookupError:
        pass
    except PermissionError as error:
        if error.errno != errno.EPERM:
            raise
        # Darwin killpg filters zombies and can return EPERM for a zombie-only
        # group. Do not suppress a genuine permission error or incomplete scan.
        # Recheck unreaped-child ownership both sides of the atomic group query.
        # Termination may be in progress between the kernel's group filtering
        # and waitid exposing the retained exit. Read only for a bounded interval;
        # never retry a denied signal or replace it with another signalling route.
        until = time.monotonic() + .5
        while True:
            if leader_exited(process) and singleton_group(process.pid) and leader_exited(process):
                return
            if time.monotonic() >= until:
                raise
            time.sleep(.01)


def stop_unreaped_group(process, grace_seconds=10):
    # Observe without reaping before any group signal. A running or zombie child
    # retains the leader PID, so this PGID cannot be reused during our cleanup.
    leader_exited(process)
    signal_owned_group(process, signal.SIGTERM)
    deadline = time.monotonic() + grace_seconds
    while not leader_exited(process) and time.monotonic() < deadline:
        time.sleep(0.05)
    # Final descendant cleanup occurs BEFORE reaping, including early leader exit.
    signal_owned_group(process, signal.SIGKILL)
    process.wait(timeout=5)
    # Never signal this PGID after wait(), even if a descendant escaped its group.
