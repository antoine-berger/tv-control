"""Existing fixture assertions, parameterized only by an owned target transport."""
import json
from proof_result import parse_proof_result, parse_activity_start

MODES = ("key-remap", "normal", "empty-null", "hint", "password", "visible-password", "selection", "focus", "stale-window", "cancel")


def run_cases(args, shell, target):
    shell("input", "keyevent", "82") # unlock owned emulator only; no audio/media controls
    # Package-manager setup can be slow on software emulation; no retry on timeout.
    # Only installation gets this budget, not instrumentation or the text engine.
    target("install", args.build / "fixture.apk", timeout=120)
    target("install", args.build / "helper.apk", timeout=120)
    settings = ("default_input_method", "selected_input_method_subtype", "enabled_accessibility_services", "accessibility_enabled")
    def snapshot():
        return tuple(shell("settings", "get", "secure", key).strip() for key in settings)
    initial = snapshot()
    result = shell("am", "instrument", "-w", "org.tvcontrol.nativehelper/org.tvcontrol.nativeinput.NativeTextInstrumentation", timeout=60)
    if "tv_native_text_v1=refused" not in result or snapshot() != initial:
        raise RuntimeError("standalone helper entrypoint/refusal proof failed")
    target("install", "-r", args.build / "proof.apk", timeout=120)
    for mode in MODES:
        shell("am", "force-stop", "org.tvcontrol.fixture")
        launch = parse_activity_start(shell("am", "start", "-W", "-n", "org.tvcontrol.fixture/.FixtureActivity", "--es", "mode", mode))
        print("Fixture launch:", mode, launch, flush=True)
        readiness_result = shell("am", "instrument", "-w", "-e", "mode", "ready",
            "org.tvcontrol.nativehelper/org.tvcontrol.nativeinput.RuntimeProof", timeout=60)
        readiness = parse_proof_result(readiness_result)
        if readiness["proof"] != "passed":
            raise RuntimeError("fixture readiness failed: " + mode + " "
                + json.dumps(readiness, sort_keys=True))
        if snapshot() != initial:
            raise RuntimeError("readiness changed IME/accessibility secure settings")
        print("PASS read-only fixture readiness:", mode, flush=True)
        if mode == "key-remap":
            shell("input", "text", "Tanya")
        result = shell("am", "instrument", "-w", "-e", "mode", mode,
                       "org.tvcontrol.nativehelper/org.tvcontrol.nativeinput.RuntimeProof", timeout=60)
        diagnostic = parse_proof_result(result)
        if diagnostic["proof"] != "passed":
            raise RuntimeError("runtime fixture assertion failed: " + mode
                + " stage=" + diagnostic["stage"] + " error=" + diagnostic["error"]
                + " synthetic_readback=" + diagnostic["readback"]
                + " overlay=" + diagnostic["overlay"]
                + " launch=" + launch + " root=" + diagnostic["root"]
                + " matches=" + diagnostic["matches"] + " report_ids=" + diagnostic["report_ids"]
                + " interactive=" + diagnostic["interactive"]
                + " keyguard_locked=" + diagnostic["keyguard_locked"]
                + " engine_status=" + diagnostic["engine_status"]
                + " replay_status=" + diagnostic["replay_status"])
        if snapshot() != initial:
            raise RuntimeError("runner changed IME/accessibility secure settings: " + mode)
        print("PASS runtime fixture:", mode, "; IME/accessibility secure settings unchanged")
    print("PASS modern helper entrypoint; all proof modes passed on owned API31 only")
