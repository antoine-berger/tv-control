package org.tvcontrol.nativeinput;

import android.system.ErrnoException;
import android.system.OsConstants;

/** One absolute monotone budget shared by the server and its accepted channel. */
final class LabDeadline {
    private final IssuedEditor.Clock clock;
    private final long absolute;
    private long last;

    LabDeadline(IssuedEditor.Clock source, long deadline) throws LabProtocol.Fault {
        if (source == null) throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);
        clock = source; last = clock.nowMillis(); absolute = deadline;
        long remaining;
        try { remaining = Math.subtractExact(deadline, last); }
        catch (ArithmeticException failure) { throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE); }
        if (last < 0 || remaining <= 0 || remaining > IssuedEditor.MAX_INSTANCE_MILLIS)
            throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);
    }
    synchronized int remaining(long until, int cap) throws LabProtocol.Fault {
        long value = clock.nowMillis();
        if (value < last || value < 0 || value >= absolute || value >= until || cap <= 0)
            throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);
        last = value;
        return (int)Math.min((long)cap, Math.min(absolute, until) - value);
    }
    synchronized long after(int millis) throws LabProtocol.Fault {
        remaining(absolute, 1);
        if (millis <= 0) throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);
        try { return Math.min(absolute, Math.addExact(last, millis)); }
        catch (ArithmeticException failure) { throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE); }
    }
    long absolute() { return absolute; }
    static boolean wouldBlock(Throwable error) {
        for (int count=0; error != null && count<8; count++, error=error.getCause()) {
            if (error instanceof ErrnoException) {
                int code=((ErrnoException)error).errno;
                return code == OsConstants.EAGAIN || code == OsConstants.EWOULDBLOCK;
            }
        }
        return false;
    }
    static LabProtocol.Fault fault(Throwable error, LabProtocol.Reason fallback) {
        for (int count=0; error != null && count<8; count++, error=error.getCause()) {
            if (error instanceof ErrnoException) {
                int code=((ErrnoException)error).errno;
                if (code == OsConstants.EPERM || code == OsConstants.EACCES)
                    return new LabProtocol.Fault(LabProtocol.Reason.POLICY_DENIED);
            }
        }
        return new LabProtocol.Fault(fallback);
    }
}
