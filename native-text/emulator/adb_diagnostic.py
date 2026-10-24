"""Fixed public diagnostics; authorized raw tails remain private local files."""
import hashlib
import os
import re
import stat

TAIL_LIMIT = 8192
PATTERNS = (
    ("ARGUMENT_SELECTOR_CONFLICT", r"-L is incompatible with -H or -P"),
    ("LISTENER_HOSTNAME_UNSUPPORTED", r"listening on specified hostname currently unsupported"),
    ("LISTENER_PERMISSION_DENIED", r"(?:could not install \*smartsocket\* listener|cannot bind listener|failed to bind[^\n]*socket)[^\n]*(?:Operation not permitted|Permission denied)"),
    ("LISTENER_ADDRESS_IN_USE", r"(?:could not install \*smartsocket\* listener|cannot bind listener|failed to bind[^\n]*socket)[^\n]*Address already in use"),
    ("INVALID_SOCKET_OR_PORT", r"(?:ADB_SERVER_SOCKET|ANDROID_ADB_SERVER_PORT|ANDROID_ADB_SERVER_ADDRESS)[^\n]*(?:invalid|must be|cannot parse)"),
    ("FOREGROUND_ARGUMENT_REJECTED", r"(?:unknown command|unknown option|usage error)[^\n]*(?:server|nodaemon)"),
    ("USB_INITIALIZATION_ERROR", r"(?:usb_init|libusb|IOKit)[^\n]*(?:failed|error|denied|not permitted)"),
    ("LOCAL_STORAGE_ERROR", r"(?:cannot|could not|failed to) (?:open|create|write)[^\n]*(?:Permission denied|Operation not permitted|No such file|No space left)"),
)
SOURCES = {"main.cpp", "adb_listeners.cpp", "commandline.cpp", "socket_spec.cpp",
           "usb_libusb.cpp", "adb_auth_host.cpp", "adb_auth.cpp", "sysdeps_unix.cpp"}


def classify(raw, total_bytes):
    text = raw.decode("utf-8", errors="replace")
    categories = [name for name, pattern in PATTERNS if re.search(pattern, text, re.I)]
    frames = []
    for name, line in re.findall(r"(?:^|[\s/])([A-Za-z_][A-Za-z_0-9]*\.cpp):(\d{1,6})(?:\D|$)", text, re.M):
        if name in SOURCES and (value := name + ":" + line) not in frames:
            frames.append(value)
            if len(frames) == 8:
                break
    return {"capture": "available", "bytes_total": total_bytes, "captured_bytes": len(raw),
            "truncated": total_bytes > len(raw), "categories": categories or ["UNKNOWN"],
            "source_frames": frames}


def private_tail(raw, directory):
    """Explicitly authorized private evidence; never emit its content anywhere."""
    # Exact bounded bytes preserve an unknown SDK failure for human inspection.
    # They are confidential even when classifiers recognize no credential form.
    data = raw[-TAIL_LIMIT:]
    directory_fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        info = os.fstat(directory_fd)
        if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
            raise ValueError("diagnostic directory is not private and owned")
        name = "adb-startup-private-tail.log"
        fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600,
                     dir_fd=directory_fd)
        with os.fdopen(fd, "wb") as target:
            target.write(data)
        return {"raw_saved": True, "raw_file": name,
                "raw_bytes": len(data), "raw_sha256": hashlib.sha256(data).hexdigest()}
    finally:
        os.close(directory_fd)


def capture(log, directory=None):
    if log is None:
        return {"capture": "not_started", "categories": ["UNKNOWN"], "source_frames": []}
    size = os.fstat(log.fileno()).st_size
    # pread never changes the shared file offset of a still-running child.
    data = os.pread(log.fileno(), min(size, TAIL_LIMIT), max(0, size - TAIL_LIMIT))
    result = classify(data, size)
    result["raw_saved"] = False
    if directory is not None:
        try:
            result.update(private_tail(data, directory))
        except (OSError, ValueError):
            result["raw_save_failed"] = True
    return result
