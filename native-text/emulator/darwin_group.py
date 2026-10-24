"""Read-only Darwin process-group proof through the public libproc API."""
import ctypes
import sys


def group_snapshot(pid):
    if sys.platform != "darwin":
        return None
    library = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    list_group = library.proc_listpgrppids
    list_group.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_int]
    list_group.restype = ctypes.c_int
    ids = (ctypes.c_int * 2)()
    ctypes.set_errno(0)
    count = list_group(pid, ids, ctypes.sizeof(ids))
    return count, list(ids), ctypes.get_errno()


def singleton_group(pid):
    # This API returns a PID count, not bytes. A full two-entry buffer cannot
    # establish completeness; one exact retained zombie is conclusive.
    snapshot = group_snapshot(pid)
    return snapshot is not None and snapshot[0] == 1 and snapshot[1][0] == pid and snapshot[2] == 0
