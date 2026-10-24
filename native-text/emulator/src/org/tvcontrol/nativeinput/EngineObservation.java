package org.tvcontrol.nativeinput;

/** Fixed diagnostics only. Does not decide or alter engine/proof behavior. */
final class EngineObservation {
    enum Status { NOT_REPORTED, VERIFIED, REFUSED, UNCERTAIN, INVALID, MULTIPLE }
    private Status initial = Status.NOT_REPORTED;
    private Status replay = Status.NOT_REPORTED;

    private static Status classify(String value) {
        if ("verified".equals(value)) return Status.VERIFIED;
        if ("refused".equals(value)) return Status.REFUSED;
        if ("uncertain".equals(value)) return Status.UNCERTAIN;
        return Status.INVALID;
    }
    void observe(String value, boolean isReplay, boolean duplicate) {
        Status result = duplicate ? Status.MULTIPLE : classify(value);
        if (isReplay) replay = result; else initial = result;
    }
    String initial() { return initial.name(); }
    String replay() { return replay.name(); }
}
