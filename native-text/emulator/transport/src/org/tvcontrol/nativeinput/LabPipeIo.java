package org.tvcontrol.nativeinput;

import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructPollfd;
import java.io.FileDescriptor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.locks.ReentrantLock;

/** Borrows process stdio for this invocation; it never closes or replaces its FD. */
final class LabPipeIo {
    enum Kind { DATA, EOF, STOP }
    static final class FrameRead {
        final Kind kind;
        private final byte[] frame;
        FrameRead(Kind value, byte[] bytes) { kind=value; frame=bytes; }
        byte[] bytes() { return frame==null ? null : frame.clone(); }
        @Override public String toString() { return "PrivatePipeFrame"; }
    }
    private final FileDescriptor borrowed;
    private final LabDeadline deadline;
    private final ReentrantLock owner=new ReentrantLock(true);
    private final ReentrantLock reader=new ReentrantLock(),writer=new ReentrantLock();
    private final AtomicBoolean stopped=new AtomicBoolean();
    private boolean prepareAttempted,prepared;
    private volatile boolean failed,stopFinished;
    private int inputBytes,inputFrames,outputBytes,outputFrames;

    LabPipeIo(FileDescriptor descriptor, IssuedEditor.Clock clock, long until) throws LabProtocol.Fault {
        if(descriptor==null)throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
        borrowed=descriptor; deadline=new LabDeadline(clock,until);
    }
    private void lock(long until) throws LabProtocol.Fault {
        try {
            if(!owner.tryLock(deadline.remaining(until,50),TimeUnit.MILLISECONDS))
                throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);
        } catch(InterruptedException error) {
            Thread.currentThread().interrupt();throw new LabProtocol.Fault(LabProtocol.Reason.CANCELLED);
        }
    }
    private void usable() throws LabProtocol.Fault {
        if(stopped.get() || failed || !prepared)throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
    }
    void prepare() throws LabProtocol.Fault {
        lock(deadline.absolute());
        try {
            if(prepareAttempted || stopped.get())throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
            prepareAttempted=true;
            int flags=Os.fcntlInt(borrowed,OsConstants.F_GETFL,0);
            Os.fcntlInt(borrowed,OsConstants.F_SETFL,flags|OsConstants.O_NONBLOCK);
            if((Os.fcntlInt(borrowed,OsConstants.F_GETFL,0)&OsConstants.O_NONBLOCK)==0)
                throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
            deadline.remaining(deadline.absolute(),1);prepared=true;
        } catch(LabProtocol.Fault error){failed=true;throw error;}
        catch(Throwable error){failed=true;throw LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST);}
        finally {owner.unlock();}
    }
    private boolean ready(short event,long until,LabSocketChannel.StopSignal stop) throws LabProtocol.Fault {
        if(stop!=null && stop.stopped())return false;
        usable();StructPollfd poll=new StructPollfd();poll.fd=borrowed;poll.events=event;
        try {Os.poll(new StructPollfd[]{poll},deadline.remaining(until,50));}
        catch(Throwable error){throw LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST);}
        if(stop!=null && stop.stopped())return false;
        usable();deadline.remaining(until,1);
        if((poll.revents&(OsConstants.POLLERR|OsConstants.POLLNVAL))!=0)
            throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
        return (poll.revents&(event|OsConstants.POLLHUP))!=0;
    }
    private Kind exact(byte[] target,int offset,int length,boolean cleanEof,long until,
            LabSocketChannel.StopSignal stop) throws LabProtocol.Fault {
        for(int position=0;position<length;) {
            if(stop!=null && stop.stopped())return Kind.STOP;
            lock(until);
            try {
                if(!ready((short)OsConstants.POLLIN,until,stop))continue;
                int count=Os.read(borrowed,target,offset+position,length-position);
                deadline.remaining(until,1);
                if(count==0) {
                    if(cleanEof && position==0)return Kind.EOF;
                    throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
                }
                if(count<0 || count>length-position)throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                inputBytes+=count;
                if(inputBytes>LabProtocol.MAX_INPUT_BYTES)throw new LabProtocol.Fault(LabProtocol.Reason.LIMIT_EXCEEDED);
                if(stop!=null && stop.stopped())return Kind.STOP;
                usable();
                position+=count;
            } catch(ErrnoException error) {
                if(!LabDeadline.wouldBlock(error))throw LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST);
            } catch(java.io.InterruptedIOException error){throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);}
            finally {owner.unlock();}
        }
        return Kind.DATA;
    }
    FrameRead readFrame(long until,LabSocketChannel.StopSignal stop) throws LabProtocol.Fault {
        if(!reader.tryLock()){failed=true;throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);}
        try {
            if(inputFrames++==LabProtocol.MAX_INPUT_FRAMES)throw new LabProtocol.Fault(LabProtocol.Reason.LIMIT_EXCEEDED);
            int initialBytes=inputBytes;
            byte[] prefix=new byte[4];Kind kind=exact(prefix,0,1,true,until,stop);
            if(kind!=Kind.DATA){
                if(kind==Kind.STOP && inputBytes!=initialBytes)throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
                if(kind==Kind.EOF)failed=true;return new FrameRead(kind,null);
            }
            long frameUntil=Math.min(until,deadline.after(2000));
            kind=exact(prefix,1,3,false,frameUntil,stop);
            if(kind==Kind.STOP)throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
            int size=LabProtocol.checkedBodyLength(((prefix[0]&255)<<24)|((prefix[1]&255)<<16)|((prefix[2]&255)<<8)|(prefix[3]&255));
            if(size>LabProtocol.MAX_INPUT_BYTES-inputBytes)throw new LabProtocol.Fault(LabProtocol.Reason.LIMIT_EXCEEDED);
            byte[] bytes=new byte[size+4];System.arraycopy(prefix,0,bytes,0,4);
            kind=exact(bytes,4,size,false,frameUntil,stop);
            if(kind==Kind.STOP)throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
            return new FrameRead(kind,bytes);
        } catch(LabProtocol.Fault error){failed=true;throw error;}
        catch(Throwable error){failed=true;throw LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST);}
        finally {reader.unlock();}
    }
    void writeFrame(byte[] value,long until) throws LabProtocol.Fault {
        if(!writer.tryLock()){failed=true;throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);}
        try {
            if(value==null || value.length<10 || value.length>LabProtocol.MAX_FRAME_BYTES
                    || value.length>LabProtocol.MAX_INPUT_BYTES-outputBytes || outputFrames++==LabProtocol.MAX_INPUT_FRAMES)
                throw new LabProtocol.Fault(LabProtocol.Reason.LIMIT_EXCEEDED);
            byte[] bytes=value.clone();
            int size=((bytes[0]&255)<<24)|((bytes[1]&255)<<16)|((bytes[2]&255)<<8)|(bytes[3]&255);
            if(LabProtocol.checkedBodyLength(size)!=bytes.length-4)throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
            int magic=((bytes[4]&255)<<24)|((bytes[5]&255)<<16)|((bytes[6]&255)<<8)|(bytes[7]&255),opcode=bytes[9]&255;
            if(magic!=LabProtocol.MAGIC || bytes[8]!=1
                    || !(opcode==1 || opcode==2 || opcode==3 || opcode==0x81 || opcode==0x82 || opcode==0x83))
                throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
            outputBytes+=bytes.length;
            for(int offset=0;offset<bytes.length;) {
                lock(until);
                try {
                    if(!ready((short)OsConstants.POLLOUT,until,null))continue;
                    int count=Os.write(borrowed,bytes,offset,bytes.length-offset);
                    deadline.remaining(until,1);usable();
                    if(count<=0 || count>bytes.length-offset)throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                    offset+=count;
                } catch(ErrnoException error) {
                    if(!LabDeadline.wouldBlock(error))throw LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST);
                } catch(java.io.InterruptedIOException error){throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);}
                finally {owner.unlock();}
            }
        } catch(LabProtocol.Fault error){failed=true;throw error;}
        catch(Throwable error){failed=true;throw LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST);}
        finally {writer.unlock();}
    }
    void stop() throws LabProtocol.Fault {
        stopped.set(true);
        boolean locked=false;
        try {
            locked=owner.tryLock(250,TimeUnit.MILLISECONDS);
            if(!locked)throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
            stopFinished=true; // No stream close, Os.close, or FD lookup/replacement.
        } catch(InterruptedException error) {
            Thread.currentThread().interrupt();throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
        } finally {if(locked)owner.unlock();}
    }
    boolean cleanupVerified(){return stopped.get() && stopFinished;}
    @Override public String toString(){return "BorrowedLabPipe";}
}
