"""Linux CI process-group cleanup with an unreaped leader protecting PGID identity."""
import os
import signal
import time


def leader_exited(process):
    # Popen.poll()/wait() would reap the leader and release its PID for reuse.
    if process.returncode is not None:
        raise RuntimeError("owned emulator leader already reaped; refusing group signals")
    try:
        info = os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
    except ChildProcessError as error:
        raise RuntimeError("owned emulator leader ownership lost; refusing group signals") from error
    return info is not None


def stop_unreaped_group(process, grace_seconds=10):
    # Observe without reaping before any group signal. A running or zombie child
    # retains the leader PID, so this PGID cannot be reused during our cleanup.
    leader_exited(process)
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    deadline = time.monotonic() + grace_seconds
    while not leader_exited(process) and time.monotonic() < deadline:
        time.sleep(0.05)
    # Final descendant cleanup occurs BEFORE reaping, including early leader exit.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait(timeout=5)
    # Never signal this PGID after wait(), even if a descendant escaped its group.
