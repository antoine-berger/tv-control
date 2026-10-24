"""Strict allowlisted diagnostic parser; never returns arbitrary instrumentation text."""
STAGES = frozenset("CONNECT_AUTOMATION WAIT_IDLE ROOT_LOOKUP EDITOR_LOOKUP READ_EDITOR REMAP_CHECK CREATE_REQUEST SERIALIZE_REQUEST FOCUS_SWITCH CANCEL_MARKER ENGINE_INVOKE ENGINE_STATUS READBACK_REFRESH READBACK_CHECK REQUEST_CONSUMED REPLAY_INVOKE REPLAY_STATUS REPLAY_READBACK CLEANUP DONE".split())
ERRORS = frozenset(("NONE", "ASSERTION", "EXCEPTION"))
READBACKS = frozenset(("NOT_CHECKED", "EXPECTED_REMAP", "UNCHANGED_LITERAL", "EMPTY", "OTHER"))
ALLOWED = {"proof": frozenset(("passed", "failed")), "stage": STAGES, "error": ERRORS, "readback": READBACKS}

ALLOWED.update(overlay=frozenset("NOT_CHECKED ANR_DIALOG CRASH_DIALOG ERROR_DIALOG GENERIC_PROMPT UNKNOWN_SYSTEM NON_SYSTEM".split()),
               root=frozenset("NOT_CHECKED FIXTURE HELPER IME SYSTEM OTHER MISSING".split()),
               matches=frozenset("NOT_CHECKED ZERO ONE MULTIPLE".split()))
for key in ("report_ids", "interactive", "keyguard_locked"):
    ALLOWED[key] = frozenset(("UNKNOWN", "YES", "NO"))
for key in ("engine_status", "replay_status"):
    ALLOWED[key] = frozenset("NOT_REPORTED VERIFIED REFUSED UNCERTAIN INVALID MULTIPLE".split())


def parse_activity_start(raw):
    """Return fixed launch categories only; never echo activity-manager output."""
    if len(raw) > 128 * 1024:
        return "INVALID"
    lines = raw.splitlines()
    if any(line.startswith(("Error:", "Error type", "Exception")) for line in lines):
        return "ERROR"
    statuses = [line[len("Status: "):] for line in lines if line.startswith("Status: ")]
    if statuses != ["ok"]:
        return "NOT_CONFIRMED"
    activities = [line[len("Activity: "):] for line in lines if line.startswith("Activity: ")]
    if activities != ["org.tvcontrol.fixture/.FixtureActivity"]:
        return "WRONG_ACTIVITY"
    return "FIXTURE_OK"


def parse_proof_result(raw):
    if len(raw) > 128 * 1024 or "INSTRUMENTATION_FAILED" in raw:
        raise RuntimeError("invalid runtime proof result")
    prefix = "INSTRUMENTATION_RESULT: tv_native_runtime_"
    result = {}
    for line in raw.splitlines():
        if not line.startswith(prefix):
            continue
        key, separator, value = line[len(prefix):].partition("=")
        if not separator or key not in ALLOWED or key in result or value not in ALLOWED[key]:
            raise RuntimeError("invalid runtime proof diagnostics")
        result[key] = value
    if result.keys() != ALLOWED.keys():
        raise RuntimeError("incomplete runtime proof diagnostics")
    if result["proof"] == "passed" and (result["stage"] != "DONE" or result["error"] != "NONE"):
        raise RuntimeError("inconsistent runtime proof diagnostics")
    return result
