"""Strict allowlisted diagnostic parser; never returns arbitrary instrumentation text."""
STAGES = frozenset("CONNECT_AUTOMATION WAIT_IDLE ROOT_LOOKUP EDITOR_LOOKUP READ_EDITOR REMAP_CHECK CREATE_REQUEST SERIALIZE_REQUEST FOCUS_SWITCH CANCEL_MARKER ENGINE_INVOKE ENGINE_STATUS READBACK_REFRESH READBACK_CHECK REQUEST_CONSUMED REPLAY_INVOKE REPLAY_STATUS REPLAY_READBACK CLEANUP DONE".split())
ERRORS = frozenset(("NONE", "ASSERTION", "EXCEPTION"))
READBACKS = frozenset(("NOT_CHECKED", "EXPECTED_REMAP", "UNCHANGED_LITERAL", "EMPTY", "OTHER"))
ALLOWED = {"proof": frozenset(("passed", "failed")), "stage": STAGES, "error": ERRORS, "readback": READBACKS}


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
