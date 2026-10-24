package org.tvcontrol.nativeinput;

import android.net.LocalSocket;
import android.net.LocalSocketAddress;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.locks.ReentrantLock;

/** One public client bind/connect; never closes a never-opened LocalSocket object. */
final class LabClientConnection {
    private final String instance;
    private final int expectedUid;
    private final LabDeadline deadline;
    private final ReentrantLock owner=new ReentrantLock(true);
    private final AtomicBoolean closeClaimed=new AtomicBoolean();
    private LocalSocket socket;
    private volatile LabSocketChannel channel;
    private boolean openAttempted, bindAttempted, rawCloseAttempted, rawCloseReturned, allocationUnknown;
    private volatile boolean closeFinished, closeFailed;

    LabClientConnection(String publicInstance, int helperUid, IssuedEditor.Clock clock, long until)
            throws LabProtocol.Fault {
        if (publicInstance==null || !publicInstance.matches("[0-9a-f]{32}"))
            throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
        if (helperUid<=0 || helperUid==2000) throw new LabProtocol.Fault(LabProtocol.Reason.PEER_UNKNOWN);
        instance=publicInstance; expectedUid=helperUid; deadline=new LabDeadline(clock,until);
    }
    LabSocketChannel open() throws LabProtocol.Fault {
        boolean locked=false;
        try {
            locked=owner.tryLock(deadline.remaining(deadline.absolute(),50),TimeUnit.MILLISECONDS);
            if (!locked) throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);
            if (openAttempted || closeClaimed.get()) throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
            openAttempted=true;
            socket=new LocalSocket(); // Public constructor only creates its Java impl, not an FD.
            bindAttempted=true;
            try {
                socket.bind(new LocalSocketAddress("tv.native.lab.client."+instance,LocalSocketAddress.Namespace.ABSTRACT));
            } catch(java.io.IOException error) {throw LabNativeSocketFailure.fromBindOrConnect(error);}
            if (socket.getFileDescriptor()==null) { allocationUnknown=true; throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST); }
            channel=new LabSocketChannel(socket,deadline); // Explicit sole-close ownership transfer.
            if (closeClaimed.get()) { channel.close(); throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST); }
            channel.prepare();
            channel.connectOnce(instance); // One call; all connect errors, including EAGAIN, stop.
            channel.admitPeer(expectedUid);
            return channel;
        } catch (LabProtocol.Fault error) { noteUnknownAllocation(); throw error; }
        catch (InterruptedException error) {
            Thread.currentThread().interrupt(); noteUnknownAllocation(); throw new LabProtocol.Fault(LabProtocol.Reason.CANCELLED);
        } catch (Throwable error) { noteUnknownAllocation(); throw LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST); }
        finally { if(locked) owner.unlock(); }
    }
    private void noteUnknownAllocation() {
        if (bindAttempted && channel==null) {
            try { if (socket==null || socket.getFileDescriptor()==null) allocationUnknown=true; }
            catch (Throwable ignored) { allocationUnknown=true; }
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
            else if (channel==null && bindAttempted && !rawCloseAttempted) {
                rawCloseAttempted=true;
                // close() lazily creates LocalSocket's FD: never call it if absent.
                if (socket==null || socket.getFileDescriptor()==null) closeFailed=true;
                else try { socket.close(); rawCloseReturned=true; } catch(Throwable error) { closeFailed=true; }
            }
        } catch (InterruptedException error) { Thread.currentThread().interrupt(); closeFailed=true; }
        finally {
            if(locked) owner.unlock();
            LabSocketChannel child=channel;
            if(child!=null) try { child.close(); } catch(LabProtocol.Fault error) { closeFailed=true; }
            closeFinished=true;
        }
        if (!cleanupVerified()) throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
    }
    boolean cleanupVerified() {
        return closeClaimed.get() && closeFinished && !closeFailed && !allocationUnknown
            && (channel!=null ? channel.cleanupVerified() : !bindAttempted || rawCloseReturned);
    }
    @Override public String toString() { return "OwnedLabClientConnection"; }
}
