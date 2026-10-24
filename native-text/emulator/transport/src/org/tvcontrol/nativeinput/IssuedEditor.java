package org.tvcontrol.nativeinput;

import java.security.MessageDigest;
import java.security.SecureRandom;

/** One lab invocation's in-memory reference and effect state. No dispatch or I/O. */
final class IssuedEditor {
    interface Clock { long nowMillis(); }
    interface Entropy { void fill(byte[] bytes); }
    enum Phase { READY, ISSUED, CONSUMED, ENGINE_ENTERED, ABORTED, CLEANING, TERMINAL }
    static final long MAX_INSTANCE_MILLIS = 15000;
    static final long REF_MILLIS = 4000;

    static final class Lease {
        private final byte[] epoch, reference;
        final long expires;
        private Lease(byte[] run, byte[] ref, long expiry) {
            epoch = run.clone(); reference = ref.clone(); expires = expiry;
        }
        byte[] epoch() { return epoch.clone(); }
        byte[] reference() { return reference.clone(); }
        @Override public String toString() { return "IssuedEditorLease"; }
    }
    static final class PendingText {
        final EditorBinding binding;
        final long expires;
        private final String after;
        private PendingText(EditorBinding value, String text, long expiry) {
            binding = value; after = text; expires = expiry;
        }
        String after() { return after; }
        @Override public String toString() { return "PendingLabText"; }
    }
    static final class Outcome {
        final LabProtocol.Status status;
        final LabProtocol.Reason reason;
        final boolean engineEntered, cleanupVerified;
        private Outcome(LabProtocol.Status result, LabProtocol.Reason why, boolean entered, boolean clean) {
            status = result; reason = why; engineEntered = entered; cleanupVerified = clean;
        }
        @Override public String toString() { return "LabOutcome"; }
    }

    private final Clock clock;
    private final Entropy entropy;
    private final byte[] epoch;
    private final long instanceDeadline;
    private long lastTime;
    private Phase phase = Phase.READY;
    private Lease lease;
    private EditorBinding binding;
    private PendingText pending;
    private boolean entered, stickyUncertain, engineReported;
    private LabProtocol.Status engineStatus;
    private LabProtocol.Reason failure;
    private Outcome terminal;

    IssuedEditor(Clock time, Entropy random, long deadline) throws LabProtocol.Fault {
        if (time == null || random == null) throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);
        clock = time; entropy = random;
        lastTime = clock.nowMillis();
        long remaining;
        try { remaining = Math.subtractExact(deadline, lastTime); }
        catch (ArithmeticException error) { throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE); }
        if (lastTime < 0 || remaining <= 0 || remaining > MAX_INSTANCE_MILLIS)
            throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);
        instanceDeadline = deadline;
        epoch = nonce();
    }
    static Entropy secureEntropy() {
        final SecureRandom random = new SecureRandom();
        return new Entropy() { public void fill(byte[] bytes) { random.nextBytes(bytes); } };
    }
    private byte[] nonce() {
        byte[] bytes = new byte[LabProtocol.NONCE_BYTES];
        entropy.fill(bytes);
        return bytes.clone();
    }
    private LabProtocol.Fault fail(LabProtocol.Reason reason) {
        abort(reason);
        return new LabProtocol.Fault(reason);
    }
    private long now() throws LabProtocol.Fault {
        long value = clock.nowMillis();
        if (value < lastTime || value < 0 || value >= instanceDeadline)
            throw fail(LabProtocol.Reason.DEADLINE);
        lastTime = value;
        return value;
    }
    synchronized Lease issue(EditorBinding observed, long observedAt) throws LabProtocol.Fault {
        if (phase != Phase.READY) throw fail(LabProtocol.Reason.REF_CONSUMED);
        long value = now(), expiry;
        if (observed == null || observedAt < 0 || observedAt > value)
            throw fail(LabProtocol.Reason.EDITOR_UNSUPPORTED);
        try { expiry = Math.min(Math.addExact(observedAt, REF_MILLIS), instanceDeadline); }
        catch (ArithmeticException error) { throw fail(LabProtocol.Reason.DEADLINE); }
        if (value >= expiry) throw fail(LabProtocol.Reason.REF_EXPIRED);
        binding = observed;
        try { lease = new Lease(epoch, nonce(), expiry); }
        catch (RuntimeException error) { throw fail(LabProtocol.Reason.TRANSPORT_LOST); }
        now(); // Entropy generation cannot extend a deadline or stale observation.
        if (lastTime >= expiry) throw fail(LabProtocol.Reason.REF_EXPIRED);
        phase = Phase.ISSUED;
        return new Lease(lease.epoch, lease.reference, lease.expires);
    }
    synchronized PendingText consume(LabProtocol.Command command) throws LabProtocol.Fault {
        if (phase != Phase.ISSUED) throw fail(LabProtocol.Reason.REF_CONSUMED);
        long value = now();
        if (command == null || command.opcode != LabProtocol.Opcode.COMMIT)
            throw fail(LabProtocol.Reason.INVALID_FRAME);
        if (!MessageDigest.isEqual(lease.epoch, command.epoch())
                || !MessageDigest.isEqual(lease.reference, command.reference()))
            throw fail(LabProtocol.Reason.REF_UNKNOWN);
        if (value >= lease.expires) throw fail(LabProtocol.Reason.REF_EXPIRED);
        String after;
        try { after = binding.append(command.append()); }
        catch (LabProtocol.Fault error) { throw fail(error.reason); }
        // Immutable absolute expiry, never now+4000 at commit.
        pending = new PendingText(binding, after, Math.min(lease.expires, instanceDeadline));
        lease = null; binding = null;
        phase = Phase.CONSUMED;
        return pending;
    }
    synchronized void markEngineEntered() throws LabProtocol.Fault {
        if (phase != Phase.CONSUMED) throw fail(LabProtocol.Reason.REF_CONSUMED);
        if (now() >= pending.expires) throw fail(LabProtocol.Reason.REF_EXPIRED);
        entered = true;
        phase = Phase.ENGINE_ENTERED;
    }
    synchronized void engineResult(String value) throws LabProtocol.Fault {
        if (!entered || engineReported || phase == Phase.CLEANING || phase == Phase.TERMINAL)
            throw fail(LabProtocol.Reason.ENGINE_UNCERTAIN);
        engineReported = true;
        if ("verified".equals(value)) engineStatus = LabProtocol.Status.VERIFIED;
        else if ("refused".equals(value)) engineStatus = LabProtocol.Status.REFUSED;
        else if ("uncertain".equals(value)) engineStatus = LabProtocol.Status.UNCERTAIN;
        else { abort(LabProtocol.Reason.ENGINE_UNCERTAIN); return; }
        // An engine result never clears an observed transport/cancellation failure.
        if (engineStatus == LabProtocol.Status.UNCERTAIN) abort(LabProtocol.Reason.ENGINE_UNCERTAIN);
    }
    synchronized void abort(LabProtocol.Reason reason) {
        if (phase == Phase.TERMINAL) return;
        if (failure == null) failure = reason == null || reason == LabProtocol.Reason.NONE
            ? LabProtocol.Reason.TRANSPORT_LOST : reason;
        stickyUncertain |= entered;
        lease = null; binding = null;
        if (phase != Phase.CLEANING) phase = Phase.ABORTED;
    }
    synchronized void beginCleanup() throws LabProtocol.Fault {
        if (phase == Phase.CLEANING || phase == Phase.TERMINAL)
            throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
        phase = Phase.CLEANING;
        lease = null; binding = null;
    }
    synchronized Outcome finishCleanup(boolean verified) throws LabProtocol.Fault {
        if (phase != Phase.CLEANING) throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
        long value = clock.nowMillis();
        if (value < lastTime || value < 0 || value >= instanceDeadline) abort(LabProtocol.Reason.DEADLINE);
        if (!verified) {
            if (failure == null) failure = LabProtocol.Reason.CLEANUP_UNVERIFIED;
            stickyUncertain = true;
        }
        if (entered && !engineReported) abort(LabProtocol.Reason.TRANSPORT_LOST);
        LabProtocol.Status status = stickyUncertain ? LabProtocol.Status.UNCERTAIN
            : entered && failure == null ? engineStatus : LabProtocol.Status.REFUSED;
        LabProtocol.Reason reason = failure != null ? failure
            : status == LabProtocol.Status.VERIFIED ? LabProtocol.Reason.NONE
            : status == LabProtocol.Status.UNCERTAIN ? LabProtocol.Reason.ENGINE_UNCERTAIN
            : entered ? LabProtocol.Reason.ENGINE_REFUSED : LabProtocol.Reason.REF_UNKNOWN;
        terminal = new Outcome(status, reason, entered, verified);
        pending = null;
        phase = Phase.TERMINAL;
        return terminal;
    }
    synchronized Phase phase() { return phase; }
    synchronized Outcome terminal() throws LabProtocol.Fault {
        if (phase != Phase.TERMINAL) throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
        return terminal;
    }
}
