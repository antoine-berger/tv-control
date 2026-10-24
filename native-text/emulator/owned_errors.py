"""Reduce bounded disposable-emulator logs to owned-error metadata; never return messages."""
import re

PACKAGES = {"org.tvcontrol.fixture": "FIXTURE", "org.tvcontrol.nativehelper": "HELPER"}
CLASSES = frozenset(("java.lang.RuntimeException", "java.lang.NullPointerException",
                    "java.lang.IllegalArgumentException", "java.lang.IllegalStateException",
                    "java.lang.SecurityException", "java.lang.ClassNotFoundException",
                    "java.lang.NoClassDefFoundError", "java.lang.VerifyError",
                    "java.lang.OutOfMemoryError", "android.view.InflateException",
                    "android.content.res.Resources$NotFoundException"))
FRAMES = {"org.tvcontrol.fixture.FixtureActivity": "FixtureActivity.java",
          "org.tvcontrol.nativeinput.RuntimeProof": "RuntimeProof.java",
          "org.tvcontrol.nativeinput.NativeTextInstrumentation": "NativeTextInstrumentation.java",
          "org.tvcontrol.nativeinput.TextEngine": "TextEngine.java"}
LINE = re.compile(r"^[EF]/(AndroidRuntime|ActivityManager)\(\s*([0-9]{1,9})\): (.*)$")


def summarize_owned_errors(crash, system):
    if len(crash) > 128 * 1024 or len(system) > 128 * 1024:
        return {"collection": "OVER_BUDGET", "crashes": [], "anrs": []}
    # A Process line binds one runtime log PID to an exact owned package. No text
    # from unrelated processes is retained, even if it contains our class names.
    records = {}
    active = {}
    for line in crash.splitlines():
        match = LINE.fullmatch(line)
        if not match or match[1] != "AndroidRuntime":
            continue
        pid, body = match[2], match[3]
        if body.startswith("FATAL EXCEPTION:"):
            active.pop(pid, None)
            continue
        process = re.fullmatch(r"Process: ([a-zA-Z0-9_.]+), PID: ([0-9]{1,9})", body)
        if process:
            active.pop(pid, None)
            if process[1] in PACKAGES and process[2] == pid:
                record = {"package": PACKAGES[process[1]], "classes": [], "frames": []}
                records[pid] = record
                active[pid] = record
            continue
        record = active.get(pid)
        if record is None:
            continue
        exception = re.match(r"^(?:Caused by: )?([a-zA-Z_$][a-zA-Z0-9_.$]*(?:Exception|Error))(?::|$)", body)
        if exception:
            name = exception[1] if exception[1] in CLASSES else "OTHER_EXCEPTION"
            if name not in record["classes"] and len(record["classes"]) < 8:
                record["classes"].append(name)
        frame = re.fullmatch(r"\s*at ([a-zA-Z0-9_.$]+)\.[a-zA-Z0-9_$<>]+\(([A-Za-z]+\.java):([0-9]{1,5})\)", body)
        if frame:
            owner = frame[1].split("$")[0]
            if FRAMES.get(owner) == frame[2] and 0 < int(frame[3]) <= 10000 and len(record["frames"]) < 8:
                # Labels and source lines only; no dynamic class/method/message output.
                value = frame[2] + ":" + str(int(frame[3]))
                if value not in record["frames"]:
                    record["frames"].append(value)
    anrs = set()
    for line in system.splitlines():
        match = LINE.fullmatch(line)
        if match and match[1] == "ActivityManager":
            anr = re.fullmatch(r"ANR in (org\.tvcontrol\.(?:fixture|nativehelper))(?: \([^\r\n]*\))?", match[3])
            if anr:
                anrs.add(PACKAGES[anr[1]])
    return {"collection": "READ", "crashes": list(records.values())[-4:], "anrs": sorted(anrs)}


def summarize_anr_windows(raw):
    """AOSP ANR window titles only, not arbitrary activity/window ownership."""
    if len(raw) > 128 * 1024:
        return {"collection": "OVER_BUDGET", "owners": ["UNKNOWN"]}
    # dumpsys can exit zero after a partial dump and append failure diagnostics.
    # Never treat an earlier matching header as complete evidence in that case.
    failure = re.compile(r"\s*(?:\*\*\* SERVICE .* DUMP TIMEOUT .*|Error(?:\b.*)|"
                         r"Permission (?:Denial|denied).*|Can't find service:.*|"
                         r"Failed to dump.*|Failed to (?:read|write).*|"
                         r"(?:read|write) failed:.*|(?:\*\*\*|\[|WARNING:|\.\.\.).*truncat.*)", re.IGNORECASE)
    if any(failure.fullmatch(line) for line in raw.splitlines()):
        return {"collection": "FAILED", "owners": ["UNKNOWN"]}
    owners = set()
    pattern = re.compile(r"\s*Window #[0-9]+ Window\{[0-9a-f]+ u[0-9]+ Application Not Responding: ([A-Za-z0-9_.:]+)\}:")
    known_system = frozenset(("system", "system_server", "android", "com.android.systemui",
                              "com.android.launcher3", "com.android.inputmethod.latin",
                              "com.android.phone", "com.android.settings"))
    for line in raw.splitlines():
        match = pattern.fullmatch(line)
        if not match:
            continue
        process = match[1]
        if process == "org.tvcontrol.fixture":
            owner = "OWNED_FIXTURE"
        elif process == "org.tvcontrol.nativehelper":
            owner = "OWNED_PROOF"
        elif process in known_system:
            owner = "SYSTEM"
        else:
            owner = "OTHER"
        owners.add(owner)
    return {"collection": "READ", "owners": sorted(owners) or ["UNKNOWN"]}
