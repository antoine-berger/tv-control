package org.tvcontrol.nativeinput;

import android.net.LocalServerSocket;
import android.net.LocalSocket;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructPollfd;
import java.io.FileDescriptor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.locks.ReentrantLock;

/** Exactly one abstract listener/accepted peer; constructor allocation ambiguity is sticky. */
final class LabSocketServer {
    private final String name;
    private final LabDeadline deadline;
    private final ReentrantLock owner=new ReentrantLock(true);
    private final AtomicBoolean closeClaimed=new AtomicBoolean();
    private LocalServerSocket socket;
    private volatile LabSocketChannel channel;
    private LocalSocket rawAccepted;
    private boolean openAttempted, listenerCloseAttempted, listenerCloseReturned;
    private boolean allocationUnknown, acceptAllocationUnknown, accepted, rawCloseAttempted, rawCloseReturned;
    private volatile boolean closeFinished, closeFailed, failed;

    LabSocketServer(String instance, IssuedEditor.Clock clock, long until) throws LabProtocol.Fault {
        if (instance == null || !instance.matches("[0-9a-f]{32}"))
            throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
        name="tv.native.lab."+instance; deadline=new LabDeadline(clock,until);
    }
    void open() throws LabProtocol.Fault {
        boolean locked=false;
        try {
            locked=owner.tryLock(deadline.remaining(deadline.absolute(),50),TimeUnit.MILLISECONDS);
            if (!locked) throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);
            if (openAttempted || closeClaimed.get()) throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
            openAttempted=true; allocationUnknown=true;
            socket=new LocalServerSocket(name);
            allocationUnknown=false;
            FileDescriptor borrowed=socket.getFileDescriptor();
            if (borrowed == null) throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
            int flags=Os.fcntlInt(borrowed,OsConstants.F_GETFL,0);
            Os.fcntlInt(borrowed,OsConstants.F_SETFL,flags|OsConstants.O_NONBLOCK);
            if ((Os.fcntlInt(borrowed,OsConstants.F_GETFL,0)&OsConstants.O_NONBLOCK)==0)
                throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
            deadline.remaining(deadline.absolute(),1);
        } catch (LabProtocol.Fault error) { failed=true; throw error; }
        catch (InterruptedException error) { failed=true; Thread.currentThread().interrupt(); throw new LabProtocol.Fault(LabProtocol.Reason.CANCELLED); }
        catch (Throwable error) { failed=true; throw LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST); }
        finally { if(locked) owner.unlock(); }
    }
    private void closeListener() throws LabProtocol.Fault {
        if (listenerCloseAttempted) {
            if (!listenerCloseReturned) throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
            return;
        }
        listenerCloseAttempted=true;
        if (socket == null) {
            if (allocationUnknown) throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
            listenerCloseReturned=true; return;
        }
        try { socket.close(); listenerCloseReturned=true; }
        catch (Throwable error) { throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED); }
    }
    LabSocketChannel accept(long until) throws LabProtocol.Fault {
        for (;;) {
            boolean locked=false;
            try {
                locked=owner.tryLock(deadline.remaining(until,50),TimeUnit.MILLISECONDS);
                if (!locked) continue;
                if (closeClaimed.get() || failed || socket == null || allocationUnknown || accepted || listenerCloseAttempted)
                    throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                StructPollfd poll=new StructPollfd(); poll.fd=socket.getFileDescriptor(); poll.events=(short)OsConstants.POLLIN;
                if (poll.fd == null) throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                Os.poll(new StructPollfd[]{poll},deadline.remaining(until,50));
                deadline.remaining(until,1);
                if (closeClaimed.get()) throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                if ((poll.revents&(OsConstants.POLLERR|OsConstants.POLLNVAL|OsConstants.POLLHUP))!=0)
                    throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                if ((poll.revents&OsConstants.POLLIN)==0) continue;
                acceptAllocationUnknown=true;
                try { rawAccepted=socket.accept(); }
                catch (Throwable error) {
                    if (LabDeadline.wouldBlock(error)) { acceptAllocationUnknown=false; continue; }
                    throw error;
                }
                if (rawAccepted == null) throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                acceptAllocationUnknown=false; accepted=true;
                channel=new LabSocketChannel(rawAccepted,deadline); // Store owner before prepare may throw.
                closeListener();
                deadline.remaining(until,1);
                if (closeClaimed.get()) { channel.close(); throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST); }
                channel.prepare();
                return channel;
            } catch (LabProtocol.Fault error) { failed=true; throw error; }
            catch (InterruptedException error) {
                failed=true; Thread.currentThread().interrupt(); throw new LabProtocol.Fault(LabProtocol.Reason.CANCELLED);
            } catch (Throwable error) { failed=true; throw LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST); }
            finally { if (locked) owner.unlock(); }
        }
    }
    void close() throws LabProtocol.Fault {
        if (!closeClaimed.compareAndSet(false,true)) {
            if (!cleanupVerified()) throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
            return;
        }
        boolean locked=false;
        try {
            locked=owner.tryLock(250,TimeUnit.MILLISECONDS);
            if (!locked) closeFailed=true;
            else {
                try { closeListener(); } catch (LabProtocol.Fault error) { closeFailed=true; }
                if (channel == null && rawAccepted != null && !rawCloseAttempted) {
                    rawCloseAttempted=true;
                    try { rawAccepted.close(); rawCloseReturned=true; } catch (Throwable error) { closeFailed=true; }
                }
            }
        } catch (InterruptedException error) { Thread.currentThread().interrupt(); closeFailed=true; }
        finally {
            if (locked) owner.unlock();
            LabSocketChannel child=channel;
            if (child != null) try { child.close(); } catch (LabProtocol.Fault error) { closeFailed=true; }
            closeFinished=true;
        }
        if (!cleanupVerified()) throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
    }
    boolean cleanupVerified() {
        return closeClaimed.get() && closeFinished && !closeFailed && !allocationUnknown
            && !acceptAllocationUnknown && listenerCloseReturned
            && (channel != null ? channel.cleanupVerified() : rawAccepted == null || rawCloseReturned);
    }
    @Override public String toString() { return "OwnedLabSocketServer"; }
}
