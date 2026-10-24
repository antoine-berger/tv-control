package org.tvcontrol.nativeinput;

import android.net.Credentials;
import android.net.LocalSocket;
import android.net.LocalSocketAddress;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructPollfd;
import java.io.FileDescriptor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.locks.ReentrantLock;

/** Accepted socket owner; all syscall descriptors are borrowed, never Os.close'd. */
final class LabSocketChannel {
    interface StopSignal { boolean stopped(); }
    enum Kind { DATA, EOF, STOP }
    static final class FrameRead {
        final Kind kind;
        private final byte[] bytes;
        private FrameRead(Kind value, byte[] frame) { kind=value; bytes=frame; }
        byte[] bytes() { return bytes == null ? null : bytes.clone(); }
        @Override public String toString() { return "PrivateLabFrameRead"; }
    }
    private final LocalSocket socket;
    private final LabDeadline deadline;
    private final ReentrantLock owner = new ReentrantLock(true);
    private final ReentrantLock reader = new ReentrantLock();
    private final ReentrantLock writer = new ReentrantLock();
    private final AtomicBoolean closeClaimed = new AtomicBoolean();
    private FileDescriptor borrowed;
    private boolean prepareAttempted, prepared, admissionAttempted, admitted, connectAttempted;
    private volatile boolean failed, closeReturned;
    private int inputBytes, inputFrames, outputBytes, outputFrames;

    LabSocketChannel(LocalSocket accepted, IssuedEditor.Clock clock, long until) throws LabProtocol.Fault {
        this(accepted, new LabDeadline(clock, until));
    }
    LabSocketChannel(LocalSocket accepted, LabDeadline budget) {
        socket=accepted; deadline=budget; // No resource allocation or API call here.
    }
    private void lock(long until) throws LabProtocol.Fault {
        try {
            if (!owner.tryLock(deadline.remaining(until, 50), TimeUnit.MILLISECONDS))
                throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt();
            throw new LabProtocol.Fault(LabProtocol.Reason.CANCELLED);
        }
    }
    private void usable(boolean needsAdmission) throws LabProtocol.Fault {
        if (closeClaimed.get() || failed || socket == null || !prepared
                || borrowed == null || socket.getFileDescriptor() != borrowed)
            throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
        if (needsAdmission && !admitted) throw new LabProtocol.Fault(LabProtocol.Reason.PEER_UNKNOWN);
    }
    void prepare() throws LabProtocol.Fault {
        lock(deadline.absolute());
        try {
            if (prepareAttempted || closeClaimed.get() || socket == null)
                throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
            prepareAttempted=true;
            borrowed=socket.getFileDescriptor();
            if (borrowed == null) throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
            int flags=Os.fcntlInt(borrowed, OsConstants.F_GETFL, 0);
            Os.fcntlInt(borrowed, OsConstants.F_SETFL, flags | OsConstants.O_NONBLOCK);
            if ((Os.fcntlInt(borrowed,OsConstants.F_GETFL,0)&OsConstants.O_NONBLOCK)==0)
                throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
            deadline.remaining(deadline.absolute(), 1);
            prepared=true;
        } catch (LabProtocol.Fault error) { failed=true; throw error; }
        catch (Throwable error) { failed=true; throw LabDeadline.fault(error, LabProtocol.Reason.TRANSPORT_LOST); }
        finally { owner.unlock(); }
    }
    void connectOnce(String instance) throws LabProtocol.Fault {
        lock(deadline.absolute());
        try {
            usable(false);
            if (connectAttempted || admissionAttempted || instance == null || !instance.matches("[0-9a-f]{32}"))
                throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
            connectAttempted=true;
            socket.connect(new LocalSocketAddress("tv.native.lab."+instance,LocalSocketAddress.Namespace.ABSTRACT));
            deadline.remaining(deadline.absolute(),1);
        } catch (LabProtocol.Fault error) { failed=true; throw error; }
        catch (Throwable error) { failed=true; throw LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST); }
        finally { owner.unlock(); }
    }
    int peerUid() throws LabProtocol.Fault {
        lock(deadline.absolute());
        try {
            usable(false);
            Credentials credentials=socket.getPeerCredentials();
            if (credentials == null || credentials.getUid() < 0)
                throw new LabProtocol.Fault(LabProtocol.Reason.PEER_UNKNOWN);
            deadline.remaining(deadline.absolute(), 1);
            return credentials.getUid();
        } catch (LabProtocol.Fault error) { failed=true; throw error; }
        catch (Throwable error) { failed=true; throw LabDeadline.fault(error, LabProtocol.Reason.PEER_UNKNOWN); }
        finally { owner.unlock(); }
    }
    void admitPeer(int expectedUid) throws LabProtocol.Fault {
        lock(deadline.absolute());
        try {
            usable(false);
            if (admissionAttempted || expectedUid <= 0)
                throw new LabProtocol.Fault(LabProtocol.Reason.PEER_MISMATCH);
            admissionAttempted=true;
            Credentials credentials=socket.getPeerCredentials();
            if (credentials == null || credentials.getUid() < 0)
                throw new LabProtocol.Fault(LabProtocol.Reason.PEER_UNKNOWN);
            if (credentials.getUid() != expectedUid)
                throw new LabProtocol.Fault(LabProtocol.Reason.PEER_MISMATCH);
            deadline.remaining(deadline.absolute(), 1);
            admitted=true;
        } catch (LabProtocol.Fault error) { failed=true; throw error; }
        catch (Throwable error) { failed=true; throw LabDeadline.fault(error, LabProtocol.Reason.PEER_UNKNOWN); }
        finally { owner.unlock(); }
    }
    private boolean ready(short event, long until, StopSignal stop) throws LabProtocol.Fault {
        if (stop != null && stop.stopped()) return false;
        usable(true);
        StructPollfd poll=new StructPollfd(); poll.fd=borrowed; poll.events=event;
        try { Os.poll(new StructPollfd[]{poll}, deadline.remaining(until, 50)); }
        catch (Throwable error) { throw LabDeadline.fault(error, LabProtocol.Reason.TRANSPORT_LOST); }
        if (stop != null && stop.stopped()) return false;
        usable(true); deadline.remaining(until, 1);
        if ((poll.revents & (OsConstants.POLLERR | OsConstants.POLLNVAL)) != 0)
            throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
        return (poll.revents & (event | OsConstants.POLLHUP)) != 0;
    }
    private Kind exact(byte[] target, int offset, int length, boolean cleanEof,
            long until, StopSignal stop) throws LabProtocol.Fault {
        int position=0;
        while (position < length) {
            if (stop != null && stop.stopped()) return Kind.STOP;
            lock(until);
            try {
                if (!ready((short)OsConstants.POLLIN, until, stop)) {
                    if (stop != null && stop.stopped()) return Kind.STOP;
                    continue;
                }
                int count=Os.read(borrowed, target, offset + position, length - position);
                deadline.remaining(until,1);
                if (stop != null && stop.stopped()) return Kind.STOP;
                usable(true);
                if (count == 0) {
                    if (cleanEof && position == 0) return Kind.EOF;
                    throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
                }
                if (count < 0 || count > length-position)
                    throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                inputBytes += count;
                if (inputBytes > LabProtocol.MAX_INPUT_BYTES)
                    throw new LabProtocol.Fault(LabProtocol.Reason.LIMIT_EXCEEDED);
                position += count;
            } catch (ErrnoException error) {
                if (!LabDeadline.wouldBlock(error)) throw LabDeadline.fault(error, LabProtocol.Reason.TRANSPORT_LOST);
            } catch (java.io.InterruptedIOException error) {
                throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
            } finally { owner.unlock(); }
        }
        return Kind.DATA;
    }
    FrameRead readFrame(long until, StopSignal stop) throws LabProtocol.Fault {
        if (!reader.tryLock()) { failed=true; throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST); }
        try {
            if (inputFrames++ == LabProtocol.MAX_INPUT_FRAMES) throw new LabProtocol.Fault(LabProtocol.Reason.LIMIT_EXCEEDED);
            byte[] prefix=new byte[4]; Kind kind=exact(prefix,0,1,true,until,stop);
            if (kind != Kind.DATA) { if (kind == Kind.EOF) failed=true; return new FrameRead(kind,null); }
            // Idle may last through the instance budget; a started frame gets
            // at most two seconds, fixed at its first byte, not reset per byte.
            long frameUntil=Math.min(until,deadline.after(2000));
            kind=exact(prefix,1,3,false,frameUntil,stop);
            if (kind == Kind.STOP) return new FrameRead(kind,null);
            int size=LabProtocol.checkedBodyLength(((prefix[0]&255)<<24)|((prefix[1]&255)<<16)|((prefix[2]&255)<<8)|(prefix[3]&255));
            if (size > LabProtocol.MAX_INPUT_BYTES-inputBytes) throw new LabProtocol.Fault(LabProtocol.Reason.LIMIT_EXCEEDED);
            byte[] frame=new byte[size+4]; System.arraycopy(prefix,0,frame,0,4);
            kind=exact(frame,4,size,false,frameUntil,stop);
            return new FrameRead(kind,kind == Kind.DATA ? frame : null);
        } catch (LabProtocol.Fault error) { failed=true; throw error; }
        catch (Throwable error) { failed=true; throw LabDeadline.fault(error, LabProtocol.Reason.TRANSPORT_LOST); }
        finally { reader.unlock(); }
    }
    void writeFrame(byte[] frame, long until) throws LabProtocol.Fault {
        if (!writer.tryLock()) { failed=true; throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST); }
        try {
            if (frame == null || frame.length < 10 || frame.length > LabProtocol.MAX_FRAME_BYTES
                    || frame.length > LabProtocol.MAX_INPUT_BYTES-outputBytes || outputFrames++ == LabProtocol.MAX_INPUT_FRAMES)
                throw new LabProtocol.Fault(LabProtocol.Reason.LIMIT_EXCEEDED);
            byte[] bytes=frame.clone();
            int size=((bytes[0]&255)<<24)|((bytes[1]&255)<<16)|((bytes[2]&255)<<8)|(bytes[3]&255);
            if (LabProtocol.checkedBodyLength(size) != bytes.length-4)
                throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
            int magic=((bytes[4]&255)<<24)|((bytes[5]&255)<<16)|((bytes[6]&255)<<8)|(bytes[7]&255);
            int opcode=bytes[9]&255;
            if (magic!=LabProtocol.MAGIC || bytes[8]!=1
                    || !(opcode==1 || opcode==2 || opcode==3 || opcode==0x81 || opcode==0x82 || opcode==0x83))
                throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
            outputBytes += bytes.length;
            for (int offset=0; offset < bytes.length;) {
                lock(until);
                try {
                    if (!ready((short)OsConstants.POLLOUT,until,null)) continue;
                    int count=Os.write(borrowed,bytes,offset,bytes.length-offset);
                    deadline.remaining(until,1);usable(true);
                    if (count <= 0 || count > bytes.length-offset) throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                    offset += count;
                } catch (ErrnoException error) {
                    if (!LabDeadline.wouldBlock(error)) throw LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST);
                } catch (java.io.InterruptedIOException error) {
                    throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                } finally { owner.unlock(); }
            }
        } catch (LabProtocol.Fault error) { failed=true; throw error; }
        catch (Throwable error) { failed=true; throw LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST); }
        finally { writer.unlock(); }
    }
    void abortIo() { failed=true; }
    void close() throws LabProtocol.Fault {
        if (!closeClaimed.compareAndSet(false,true)) {
            if (!closeReturned) throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
            return;
        }
        boolean locked=false;
        try {
            locked=owner.tryLock(250,TimeUnit.MILLISECONDS);
            if (!locked || socket == null || socket.getFileDescriptor()==null)
                throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
            socket.close(); closeReturned=true; borrowed=null;
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt(); throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
        } catch (Throwable error) { throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED); }
        finally { if (locked) owner.unlock(); }
    }
    boolean cleanupVerified() { return closeClaimed.get() && closeReturned; }
    @Override public String toString() { return "OwnedLabSocketChannel"; }
}
