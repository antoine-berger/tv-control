package org.tvcontrol.nativeinput;

import java.io.File;

/** Common wrapper owner: transport hooks cannot publish or upgrade its terminal result. */
final class NativePendingRequest {
    interface Lifecycle {
        boolean beforeEntry() throws Exception;
        void result(String status) throws Exception;
        void failed();
        void close() throws Exception;
        boolean cleanupVerified() throws Exception;
        String terminalStatus() throws Exception;
        String terminalReason() throws Exception;
    }
    enum Cleanup { NOT_APPLICABLE, VERIFIED, UNVERIFIED }
    enum Reason { NONE, PEER_UNKNOWN, PEER_MISMATCH, POLICY_DENIED, IDENTITY_CHANGED,
        INVALID_FRAME, LIMIT_EXCEEDED, EDITOR_UNSUPPORTED, REF_UNKNOWN, REF_CONSUMED,
        REF_EXPIRED, CANCELLED, DEADLINE, ENGINE_REFUSED, ENGINE_UNCERTAIN,
        TRANSPORT_LOST, CLEANUP_UNVERIFIED }
    static final class Terminal {
        final String status, publicInstance;
        final Cleanup cleanup;
        final Reason reason;
        final boolean entered;
        private Terminal(String value, Cleanup clean, String instance, Reason why, boolean engineEntered) {
            status = value; cleanup = clean; publicInstance = instance; reason = why; entered = engineEntered;
        }
        @Override public String toString() { return "NativeTerminalResult"; }
    }
    private File cache;
    private String token;
    private String publicInstance;
    private Lifecycle lifecycle;
    private boolean cacheSet, tokenSet, entered, reported, finished, uncertain;
    private String engineStatus = "refused";
    private Reason failure;

    void cache(File value) {
        if (cacheSet || finished || value == null) throw new IllegalStateException("pending cache state");
        cache = value; cacheSet = true;
    }
    File cache() { return cache; }
    void token(String value) {
        if (tokenSet || entered || finished) throw new IllegalStateException("pending token state");
        token = value; tokenSet = true;
    }
    String token() { return token; }
    void publicInstance(String value) {
        if (publicInstance != null || lifecycle != null || entered || finished
                || value == null || !value.matches("[0-9a-f]{32}"))
            throw new IllegalStateException("pending invocation state");
        // Public rendezvous metadata only; never a payload/ref/cache token.
        publicInstance = value;
    }
    void attach(Lifecycle value) {
        if (lifecycle != null || entered || finished || value == null || publicInstance == null)
            throw new IllegalStateException("pending lifecycle state");
        lifecycle = value;
    }
    boolean enter() throws Exception {
        if (entered || finished || !cacheSet || !tokenSet)
            throw new IllegalStateException("pending entry state");
        if (lifecycle != null && !lifecycle.beforeEntry()) return false;
        entered = true;
        return true;
    }
    private static boolean known(String value) {
        return "refused".equals(value) || "uncertain".equals(value) || "verified".equals(value);
    }
    void result(String value) {
        if (!entered || reported || finished) { markUncertain(Reason.ENGINE_UNCERTAIN); failed(); return; }
        reported = true;
        engineStatus = known(value) ? value : "uncertain";
        if ("uncertain".equals(engineStatus)) markUncertain(Reason.ENGINE_UNCERTAIN);
        if (lifecycle != null) {
            try { lifecycle.result(engineStatus); }
            catch (Throwable failure) { failed(); }
        }
    }
    void failed() {
        markUncertain(Reason.TRANSPORT_LOST);
        if (lifecycle != null) {
            try { lifecycle.failed(); }
            catch (Throwable ignored) { /* Sticky uncertain; no raw exception output. */ }
        }
    }
    private void markUncertain(Reason value) {
        uncertain = true;
        if (failure == null) failure = value;
    }
    Terminal finish() {
        if (finished) return new Terminal("uncertain", Cleanup.UNVERIFIED, publicInstance, Reason.CLEANUP_UNVERIFIED, entered);
        finished = true;
        String transportStatus = engineStatus;
        Cleanup cleanup = Cleanup.NOT_APPLICABLE;
        Reason reason = "verified".equals(engineStatus) ? Reason.NONE
            : "uncertain".equals(engineStatus) ? Reason.ENGINE_UNCERTAIN : Reason.ENGINE_REFUSED;
        if (lifecycle != null) {
            cleanup = Cleanup.UNVERIFIED;
            boolean closeSucceeded = true, closed = false;
            try { lifecycle.close(); }
            catch (Throwable error) { closeSucceeded = false; markUncertain(Reason.CLEANUP_UNVERIFIED); }
            // A hook must explicitly attest its own remote resources after close.
            // Local adb/am/client exit cannot supply this proof.
            try { closed = lifecycle.cleanupVerified(); }
            catch (Throwable error) { markUncertain(Reason.CLEANUP_UNVERIFIED); }
            if (closed && closeSucceeded) cleanup = Cleanup.VERIFIED;
            else markUncertain(Reason.CLEANUP_UNVERIFIED);
            try { transportStatus = lifecycle.terminalStatus(); }
            catch (Throwable error) { markUncertain(Reason.TRANSPORT_LOST); }
            try { reason = Reason.valueOf(lifecycle.terminalReason()); }
            catch (Throwable error) { markUncertain(Reason.TRANSPORT_LOST); }
        }
        if (!known(transportStatus) || entered && !reported) markUncertain(Reason.TRANSPORT_LOST);
        if ("verified".equals(transportStatus)
                && (!entered || !reported || !"verified".equals(engineStatus))) markUncertain(Reason.TRANSPORT_LOST);
        if ("verified".equals(engineStatus) && "refused".equals(transportStatus)) markUncertain(Reason.TRANSPORT_LOST);
        if (entered && "refused".equals(transportStatus)
                && (!"refused".equals(engineStatus) || reason != Reason.ENGINE_REFUSED))
            markUncertain(reason == Reason.NONE ? Reason.TRANSPORT_LOST : reason);
        if (entered && reason != Reason.NONE && reason != Reason.ENGINE_REFUSED)
            markUncertain(reason);
        if ("verified".equals(transportStatus) && reason != Reason.NONE) markUncertain(reason);
        if ("uncertain".equals(transportStatus) && reason == Reason.NONE) reason = Reason.ENGINE_UNCERTAIN;
        cache = null; token = null; lifecycle = null;
        if (failure != null) reason = failure;
        else if (uncertain && reason == Reason.NONE) reason = Reason.TRANSPORT_LOST;
        return new Terminal(uncertain || "uncertain".equals(transportStatus)
            ? "uncertain" : transportStatus, cleanup, publicInstance, reason, entered);
    }
    @Override public String toString() { return "NativePendingRequest"; }
}
