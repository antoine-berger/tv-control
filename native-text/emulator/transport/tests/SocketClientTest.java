package org.tvcontrol.nativeinput;

import android.net.LocalSocket;
import android.system.Os;
import android.system.OsConstants;
import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.io.FileDescriptor;
import java.util.Arrays;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** Actual client/pipe classes; every Android syscall below is a host fake. */
public final class SocketClientTest {
    private static final String INSTANCE="0123456789abcdef0123456789abcdef";
    private static int checks;
    interface Attempt {void run()throws Exception;}
    private static void check(boolean value){checks++;if(!value)throw new AssertionError("fixed client check "+checks);}
    private static void fault(LabProtocol.Reason reason,Attempt task)throws Exception {
        try{task.run();throw new AssertionError("missing fixed fault");}
        catch(LabProtocol.Fault error){check(error.reason==reason);}
    }
    private static final class Clock implements IssuedEditor.Clock {
        long value=1;public long nowMillis(){return value;}
    }
    private static void reset(){Os.reset();LocalSocket.reset();android.os.SystemClock.offset=0;}
    private static byte[] frame(int opcode)throws Exception {
        ByteArrayOutputStream bytes=new ByteArrayOutputStream();DataOutputStream out=new DataOutputStream(bytes);
        out.writeInt(6);out.writeInt(LabProtocol.MAGIC);out.writeByte(1);out.writeByte(opcode);return bytes.toByteArray();
    }
    private static byte[] reference()throws Exception {
        ByteArrayOutputStream bytes=new ByteArrayOutputStream();DataOutputStream out=new DataOutputStream(bytes);
        out.writeInt(46);out.writeInt(LabProtocol.MAGIC);out.writeByte(1);out.writeByte(0x81);
        out.write(new byte[16]);byte[] ref=new byte[16];Arrays.fill(ref,(byte)1);out.write(ref);
        out.writeLong(android.os.SystemClock.uptimeMillis()+4000);return bytes.toByteArray();
    }
    private static byte[] commit(byte[] reference)throws Exception {
        ByteArrayOutputStream bytes=new ByteArrayOutputStream();DataOutputStream out=new DataOutputStream(bytes);
        out.writeInt(43);out.writeInt(LabProtocol.MAGIC);out.writeByte(1);out.writeByte(2);
        out.write(reference,10,32);out.writeInt(1);out.writeByte('x');return bytes.toByteArray();
    }
    private static int frameCount(byte[] bytes) {
        int count=0,offset=0;
        while(bytes.length-offset>=4) {
            int size=((bytes[offset]&255)<<24)|((bytes[offset+1]&255)<<16)|((bytes[offset+2]&255)<<8)|(bytes[offset+3]&255);
            if(size<6 || size>16380 || bytes.length-offset<size+4)return count;
            count++;offset+=size+4;
        }
        return count;
    }
    private static int run(){return LabSocketClient.run(new String[]{INSTANCE,"10001"});}
    private static void noBorrowedClose(){check(Os.directCloses==0);check(Os.state(FileDescriptor.in).closes==0);check(Os.state(FileDescriptor.out).closes==0);}
    private static void noWriter(){
        for(Thread thread:Thread.getAllStackTraces().keySet())check(!thread.getName().equals("native-lab-stdin") || !thread.isAlive());
    }
    private static void clientPositive(boolean cancel)throws Exception {
        reset();byte[] first=frame(cancel?3:1);Os.append(FileDescriptor.in,first);
        Os.writeHook=()->{
            FileDescriptor fd=Os.connected;
            if(fd!=null && frameCount(Os.output(fd))==1)Os.eof(fd);
        };
        check(run()==0);check(Arrays.equals(Os.output(Os.connected),first));
        check(Arrays.equals(Os.output(FileDescriptor.out),LabReplies.channelClosed()));
        check(LocalSocket.binds==1 && LocalSocket.connects==1 && LocalSocket.closes==1);
        check(!Os.state(FileDescriptor.in).eof);noBorrowedClose();noWriter();
        check(Os.output(FileDescriptor.err).length==0); // Success never emits a diagnostic.
    }
    private static void commitKeepsInputOpen(boolean closeInput,boolean lateMalformed)throws Exception {
        reset();byte[] ref=reference(),commit=commit(ref);Os.append(FileDescriptor.in,frame(1));
        boolean[] emitted={false,false};
        Os.writeHook=()->{
            FileDescriptor fd=Os.connected;if(fd==null)return;
            if(frameCount(Os.output(fd))==1 && !emitted[0]){emitted[0]=true;Os.append(fd,ref);}
            if(frameCount(Os.output(FileDescriptor.out))==1 && !emitted[1]) {
                emitted[1]=true;Os.append(FileDescriptor.in,commit);
                if(closeInput)Os.eof(FileDescriptor.in);
                if(lateMalformed)Os.append(FileDescriptor.in,new byte[]{0,0,0,0});
            }
            if(frameCount(Os.output(fd))==2 && !closeInput && !lateMalformed)Os.eof(fd);
        };
        check(run()==(closeInput || lateMalformed?1:0));check(frameCount(Os.output(Os.connected))==2);
        check(frameCount(Os.output(FileDescriptor.out))==(closeInput || lateMalformed?1:2));
        if(!closeInput && !lateMalformed)check(Os.output(FileDescriptor.err).length==0);
        noBorrowedClose();noWriter();
    }
    private static void admissionAndProtocolFailures()throws Exception {
        reset();Os.uid=0;check(run()==1);check(LocalSocket.constructors==0);noBorrowedClose();
        reset();Os.euid=0;check(run()==1);check(LocalSocket.constructors==0);
        for(String uid:new String[]{"0","2000","010001","+10001","-1","2147483648","10001\n"}) {
            reset();check(LabSocketClient.run(new String[]{INSTANCE,uid})==1);check(LocalSocket.constructors==0);
        }
        reset();check(LabSocketClient.run(new String[]{INSTANCE,"10001","payload"})==1);check(LocalSocket.constructors==0);
        reset();LocalSocket.peerUid=10002;Os.append(FileDescriptor.in,frame(1));check(run()==1);
        check(Os.state(FileDescriptor.in).reads==0);check(Os.output(Os.connected).length==0);check(LocalSocket.closes==1);
        reset();LocalSocket.onConnect=()->Os.eof(Os.connected);check(run()==1);check(Os.output(FileDescriptor.out).length==0);noWriter();
        reset();Os.eof(FileDescriptor.in);check(run()==1);check(Os.output(FileDescriptor.out).length==0);noWriter();
        reset();Os.append(FileDescriptor.in,frame(0x81));check(run()==1);check(Os.output(Os.connected).length==0);noWriter();
        reset();Os.append(FileDescriptor.in,frame(2));check(run()==1);check(Os.output(Os.connected).length==0);
        reset();Os.append(FileDescriptor.in,frame(1));Os.append(FileDescriptor.in,frame(1));check(run()==1);
        check(frameCount(Os.output(Os.connected))==1);noWriter();
        reset();LocalSocket.connectErrno=OsConstants.EAGAIN;check(run()==1);check(LocalSocket.connects==1 && LocalSocket.closes==1);
        reset();Os.state(FileDescriptor.in).ignoreFlags=true;check(run()==1);check(LocalSocket.closes==1);noBorrowedClose();
    }
    private static void replyFailures()throws Exception {
        reset();byte[] ref=reference();Os.append(FileDescriptor.in,frame(1));boolean[] sent={false};
        Os.writeHook=()->{
            if(Os.connected!=null && frameCount(Os.output(Os.connected))==1 && !sent[0]) {
                sent[0]=true;Os.append(Os.connected,ref);Os.append(Os.connected,ref);
            }
        };
        check(run()==1);check(frameCount(Os.output(FileDescriptor.out))==1);noWriter();
        reset();Os.append(FileDescriptor.in,frame(1));LocalSocket.closeFailure=true;
        Os.writeHook=()->{if(Os.connected!=null && frameCount(Os.output(Os.connected))==1)Os.eof(Os.connected);};
        check(run()==2);check(Os.output(FileDescriptor.out).length==0);check(LocalSocket.closes==1);noWriter();
        reset();Os.append(FileDescriptor.in,frame(1));Os.state(FileDescriptor.out).writeErrno=OsConstants.EACCES;
        Os.writeHook=()->{if(Os.connected!=null && frameCount(Os.output(Os.connected))==1)Os.eof(Os.connected);};
        check(run()==1);check(Os.output(FileDescriptor.out).length==0);noBorrowedClose();noWriter();
        reset();byte[] early=reference();LocalSocket.onConnect=()->Os.append(Os.connected,early);
        check(run()==1);check(Os.output(FileDescriptor.out).length==0);noWriter();
        for(int mode=0;mode<3;mode++) {
            reset();Os.append(FileDescriptor.in,frame(1));byte[] bad=reference();
            if(mode==0)bad[9]=(byte)0x82;
            else if(mode==1)Arrays.fill(bad,42,50,(byte)0);
            else bad=Arrays.copyOf(bad,49);
            final byte[] invalid=bad;final boolean[] once={false};
            Os.writeHook=()->{
                if(Os.connected!=null && frameCount(Os.output(Os.connected))==1 && !once[0]){
                    once[0]=true;Os.append(Os.connected,invalid);Os.eof(Os.connected);
                }
            };
            check(run()==1);check(Os.output(FileDescriptor.out).length==0);noWriter();
        }
        reset();byte[] lease=reference(),wrong=commit(lease);wrong[10]=1;Os.append(FileDescriptor.in,frame(1));
        boolean[] progressed={false,false};
        Os.writeHook=()->{
            if(Os.connected==null)return;
            if(frameCount(Os.output(Os.connected))==1 && !progressed[0]){progressed[0]=true;Os.append(Os.connected,lease);}
            if(frameCount(Os.output(FileDescriptor.out))==1 && !progressed[1]){progressed[1]=true;Os.append(FileDescriptor.in,wrong);}
        };
        check(run()==1);check(frameCount(Os.output(Os.connected))==1);noWriter();
    }
    private static void pipeBoundaries()throws Exception {
        reset();final Clock clock=new Clock();Os.advance=n->clock.value+=n;
        LabPipeIo pipe=new LabPipeIo(FileDescriptor.in,clock,15001);pipe.prepare();
        Os.append(FileDescriptor.in,frame(1));byte[] bytes=pipe.readFrame(15001,()->false).bytes();
        check(Arrays.equals(bytes,frame(1)));bytes[0]=1;pipe.stop();check(pipe.cleanupVerified());
        int oldPolls=Os.state(FileDescriptor.in).polls;Os.states.put(FileDescriptor.in,new Os.State());
        fault(LabProtocol.Reason.TRANSPORT_LOST,()->pipe.readFrame(15001,()->false));
        check(Os.state(FileDescriptor.in).polls==0);check(oldPolls>0);noBorrowedClose();
        reset();final Clock c=new Clock();Os.advance=n->c.value+=n;
        LabPipeIo slow=new LabPipeIo(FileDescriptor.in,c,15001);slow.prepare();
        Os.append(FileDescriptor.in,new byte[]{0});
        fault(LabProtocol.Reason.DEADLINE,()->slow.readFrame(15001,()->false));
        check(c.value==2001);slow.stop();check(slow.cleanupVerified());
        reset();Clock idleClock=new Clock();Os.advance=n->idleClock.value+=n;
        LabPipeIo idle=new LabPipeIo(FileDescriptor.in,idleClock,15001);idle.prepare();
        check(idle.readFrame(15001,()->idleClock.value>=5001).kind==LabPipeIo.Kind.STOP);
        check(idleClock.value>=5001 && idleClock.value<5100);idle.stop();noBorrowedClose();
        reset();Clock partialClock=new Clock();Os.advance=n->partialClock.value+=n;
        LabPipeIo partial=new LabPipeIo(FileDescriptor.in,partialClock,15001);partial.prepare();
        Os.append(FileDescriptor.in,new byte[]{0});
        fault(LabProtocol.Reason.INVALID_FRAME,()->partial.readFrame(15001,()->partialClock.value>=51));
        partial.stop();check(partial.cleanupVerified());
        reset();Clock lastClock=new Clock();Os.advance=n->lastClock.value+=n;
        LabPipeIo late=new LabPipeIo(FileDescriptor.in,lastClock,15001);late.prepare();
        Os.append(FileDescriptor.in,frame(1));Os.State input=Os.state(FileDescriptor.in);input.readAdvanceAfter=4;input.readAdvance=2000;
        fault(LabProtocol.Reason.DEADLINE,()->late.readFrame(15001,()->false));late.stop();
        reset();Clock outClock=new Clock();Os.advance=n->outClock.value+=n;
        LabPipeIo output=new LabPipeIo(FileDescriptor.out,outClock,15001);output.prepare();
        Os.state(FileDescriptor.out).writeErrno=OsConstants.EAGAIN;output.writeFrame(frame(3),1000);
        check(Arrays.equals(Os.output(FileDescriptor.out),frame(3)));output.stop();noBorrowedClose();
        reset();LabPipeIo invalid=new LabPipeIo(FileDescriptor.out,new Clock(),15001);invalid.prepare();
        byte[] wrongMagic=frame(3);wrongMagic[4]=0;
        fault(LabProtocol.Reason.INVALID_FRAME,()->invalid.writeFrame(wrongMagic,1000));
        check(Os.state(FileDescriptor.out).writes==0);invalid.stop();
        reset();LabPipeIo denied=new LabPipeIo(FileDescriptor.out,new Clock(),15001);denied.prepare();
        Os.state(FileDescriptor.out).writeErrno=OsConstants.EACCES;
        fault(LabProtocol.Reason.POLICY_DENIED,()->denied.writeFrame(frame(3),1000));denied.stop();noBorrowedClose();
    }
    private static void stopCannotCloseReusedFd()throws Exception {
        reset();Clock clock=new Clock();LabPipeIo pipe=new LabPipeIo(FileDescriptor.in,clock,15001);pipe.prepare();
        CountDownLatch entered=new CountDownLatch(1),release=new CountDownLatch(1);
        Os.pollHook=()->{entered.countDown();try{release.await(2,TimeUnit.SECONDS);}catch(InterruptedException error){Thread.currentThread().interrupt();}};
        Thread reader=new Thread(()->{try{pipe.readFrame(15001,()->false);}catch(LabProtocol.Fault expected){}});
        reader.start();check(entered.await(1,TimeUnit.SECONDS));
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED,()->pipe.stop());check(!pipe.cleanupVerified());
        release.countDown();reader.join(1000);check(!reader.isAlive());
        Os.states.put(FileDescriptor.in,new Os.State());pipe.stop();check(pipe.cleanupVerified());
        check(Os.state(FileDescriptor.in).polls==0);noBorrowedClose();
    }
    private static void clientUnjoinedWriterIsNotClean()throws Exception {
        reset();Os.append(FileDescriptor.in,frame(1));
        CountDownLatch entered=new CountDownLatch(1),release=new CountDownLatch(1);
        Os.pollEach=fd->{
            if(fd==FileDescriptor.in && Os.state(fd).position==10) {
                entered.countDown();
                try{release.await(3,TimeUnit.SECONDS);}catch(InterruptedException ignored){
                    // Model a native poll that does not honor Java interrupt.
                    try{release.await(3,TimeUnit.SECONDS);}catch(InterruptedException again){Thread.currentThread().interrupt();}
                }
            }
        };
        Thread server=new Thread(()->{
            try{if(entered.await(1,TimeUnit.SECONDS))Os.eof(Os.connected);}
            catch(InterruptedException error){Thread.currentThread().interrupt();}
        });
        server.start();int result=run();
        check(result==2);check(Os.output(FileDescriptor.out).length==0);release.countDown();server.join(1000);
        for(Thread thread:Thread.getAllStackTraces().keySet())if(thread.getName().equals("native-lab-stdin"))thread.join(1000);
        noWriter();noBorrowedClose();
    }
    private static void decodedLateFrameCannotBeHiddenByStop(int mode)throws Exception {
        reset();IssuedEditor.Clock clock=()->android.os.SystemClock.uptimeMillis();long until=clock.nowMillis()+15000;
        LabClientConnection connection=new LabClientConnection(INSTANCE,10001,clock,until);
        LabSocketChannel channel=connection.open();LabPipeIo input=new LabPipeIo(FileDescriptor.in,clock,until);input.prepare();
        byte[] ref=reference();Os.append(FileDescriptor.in,mode==2?commit(ref):frame(mode==1?3:1));
        LabSocketClient.ForwardState state=new LabSocketClient.ForwardState();
        state.phase=mode==1?LabProtocol.Opcode.COMMIT:LabProtocol.Opcode.OBSERVE;
        if(mode==2){state.reference=LabReplies.parseReference(ref);state.referenceSent=true;}
        LabSocketClient.Forwarder forwarder=new LabSocketClient.Forwarder(input,channel,new LabDeadline(clock,until),state);
        Thread thread=new Thread(forwarder,"owned-fake-forwarder");
        synchronized(state) {
            thread.start();long end=System.nanoTime()+TimeUnit.SECONDS.toNanos(1);
            while(thread.getState()!=Thread.State.BLOCKED && System.nanoTime()<end)Thread.yield();
            check(thread.getState()==Thread.State.BLOCKED);forwarder.stop.set(true);
        }
        thread.join(1000);check(!thread.isAlive());
        check(forwarder.failure.get()==(mode==0?LabProtocol.Reason.INVALID_FRAME
                :mode==1?LabProtocol.Reason.CANCELLED:LabProtocol.Reason.TRANSPORT_LOST));
        check(Os.output(Os.connected).length==0);input.stop();connection.close();noBorrowedClose();
    }
    private static void stopDuringPartialCommit(boolean fenceTimeout)throws Exception {
        reset();IssuedEditor.Clock clock=()->android.os.SystemClock.uptimeMillis();long until=clock.nowMillis()+15000;
        LabClientConnection connection=new LabClientConnection(INSTANCE,10001,clock,until);
        LabSocketChannel channel=connection.open();LabPipeIo input=new LabPipeIo(FileDescriptor.in,clock,until);input.prepare();
        byte[] ref=reference(),request=commit(ref);Os.append(FileDescriptor.in,request);
        LabSocketClient.ForwardState state=new LabSocketClient.ForwardState();
        state.phase=LabProtocol.Opcode.OBSERVE;state.reference=LabReplies.parseReference(ref);state.referenceSent=true;
        LabSocketClient.Forwarder forwarder=new LabSocketClient.Forwarder(input,channel,new LabDeadline(clock,until),state);
        CountDownLatch entered=new CountDownLatch(1),release=new CountDownLatch(1);
        Os.writeHook=()->{
            if(Os.output(Os.connected).length==3) {
                entered.countDown();
                try{release.await(2,TimeUnit.SECONDS);}catch(InterruptedException error){Thread.currentThread().interrupt();}
            }
        };
        Thread worker=new Thread(forwarder,"owned-partial-commit");boolean[] stopped={false};
        Thread stopper=new Thread(()->stopped[0]=forwarder.stopAndJoin(worker),"owned-client-stopper");
        worker.start();check(entered.await(1,TimeUnit.SECONDS));stopper.start();
        if(fenceTimeout) {
            stopper.join(1500);check(!stopper.isAlive());
            check(!stopped[0]);check(forwarder.stop.get());
            check(forwarder.failure.get()==LabProtocol.Reason.CLEANUP_UNVERIFIED);
            release.countDown();
        } else {
            long end=System.nanoTime()+TimeUnit.MILLISECONDS.toNanos(150);
            while(stopper.getState()!=Thread.State.TIMED_WAITING && System.nanoTime()<end)Thread.yield();
            check(stopper.getState()==Thread.State.TIMED_WAITING);check(!forwarder.stop.get());
            release.countDown();stopper.join(1000);check(!stopper.isAlive());check(stopped[0]);
        }
        worker.join(1000);check(!worker.isAlive());
        check(Os.output(Os.connected).length==3);check(Os.state(Os.connected).writes==1);
        check(state.phase==LabProtocol.Opcode.OBSERVE);
        check(forwarder.failure.get()==(fenceTimeout?LabProtocol.Reason.CLEANUP_UNVERIFIED:LabProtocol.Reason.TRANSPORT_LOST));
        input.stop();connection.close();noBorrowedClose();
    }
    public static void main(String[] args)throws Exception {
        clientPositive(false);clientPositive(true);
        commitKeepsInputOpen(false,false);commitKeepsInputOpen(true,false);commitKeepsInputOpen(false,true);
        admissionAndProtocolFailures();replyFailures();pipeBoundaries();stopCannotCloseReusedFd();clientUnjoinedWriterIsNotClean();
        decodedLateFrameCannotBeHiddenByStop(0);decodedLateFrameCannotBeHiddenByStop(1);decodedLateFrameCannotBeHiddenByStop(2);
        stopDuringPartialCommit(false);stopDuringPartialCommit(true);
        System.out.println("PASS private client host checks: "+checks);
    }
}
