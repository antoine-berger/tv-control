package org.tvcontrol.nativeinput;

import android.net.LocalServerSocket;
import android.net.LocalSocket;
import android.system.Os;
import android.system.OsConstants;
import java.io.FileDescriptor;
import java.nio.ByteBuffer;
import java.util.Arrays;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** Actual owners with fake Android APIs only; no real socket, FD or Android runtime. */
public final class SocketIoTest {
    static int checks;
    static final String INSTANCE="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    interface Checked { void run() throws Exception; }
    static final class Clock implements IssuedEditor.Clock {
        long time=100;
        public long nowMillis() { return time; }
    }
    static Clock reset() {
        Os.states.clear();Os.nextNumeric=5;Os.directCloses=0;Os.pollHook=null;Os.ignoreAllFlagSets=false;
        LocalServerSocket.queue.clear();LocalServerSocket.constructors=0;LocalServerSocket.accepts=0;
        LocalServerSocket.constructorErrno=0;LocalServerSocket.acceptUnknown=false;LocalServerSocket.spuriousReady=false;
        LocalServerSocket.last=null;LocalServerSocket.lastAllocated=null;
        LocalSocket.constructors=0;LocalSocket.binds=0;LocalSocket.connects=0;LocalSocket.lazyCloseAllocations=0;
        LocalSocket.bindErrno=0;LocalSocket.connectErrno=0;LocalSocket.bindNoFd=false;
        LocalSocket.lastBind=null;LocalSocket.lastConnect=null;
        Clock clock=new Clock();Os.advance=amount -> clock.time+=amount;return clock;
    }
    static void check(boolean value) { checks++;if(!value)throw new AssertionError("socket-owner invariant"); }
    static void fault(LabProtocol.Reason reason,Checked call) throws Exception {
        try { call.run(); } catch(LabProtocol.Fault expected) {
            check(expected.reason==reason);check(expected.getMessage().equals(reason.name()));return;
        }
        throw new AssertionError("expected fixed socket refusal");
    }
    static byte[] observe() { return ByteBuffer.allocate(10).putInt(6).putInt(LabProtocol.MAGIC).put((byte)1).put((byte)1).array(); }
    static LabSocketChannel channel(LocalSocket socket,Clock clock) throws Exception {
        LabSocketChannel value=new LabSocketChannel(socket,clock,15100);value.prepare();value.admitPeer(2000);return value;
    }
    static void constructorAndPeer() throws Exception {
        Clock clock=reset();LabSocketServer server=new LabSocketServer(INSTANCE,clock,15100);
        check(LocalServerSocket.constructors==0);server.open();check(LocalServerSocket.constructors==1);
        LocalSocket wrong=LocalSocket.accepted(999,observe());FileDescriptor fd=wrong.getFileDescriptor();
        LocalServerSocket.queue.add(wrong);LabSocketChannel peer=server.accept(5100);
        check(LocalServerSocket.accepts==1 && LocalServerSocket.last.closeCalls==1);
        final LabSocketChannel unadmitted=peer;
        fault(LabProtocol.Reason.PEER_UNKNOWN,() -> unadmitted.readFrame(2100,null));
        check(Os.state(fd).reads==0);
        // A fresh instance: the earlier unauthorized data attempt poisoned that channel.
        server.close();check(server.cleanupVerified());check(wrong.closeCalls==1);
        clock=reset();server=new LabSocketServer(INSTANCE,clock,15100);server.open();
        wrong=LocalSocket.accepted(999,observe());fd=wrong.getFileDescriptor();LocalServerSocket.queue.add(wrong);
        peer=server.accept(5100);final LabSocketChannel denied=peer;
        fault(LabProtocol.Reason.PEER_MISMATCH,() -> denied.admitPeer(2000));
        fault(LabProtocol.Reason.TRANSPORT_LOST,() -> denied.admitPeer(999));
        final LabSocketServer one=server;
        fault(LabProtocol.Reason.TRANSPORT_LOST,() -> one.accept(5100));
        check(LocalServerSocket.accepts==1 && Os.state(fd).reads==0);
        server.close();peer.close();server.close();
        check(server.cleanupVerified() && wrong.closeCalls==1 && Os.directCloses==0);
        clock=reset();server=new LabSocketServer(INSTANCE,clock,15100);LocalServerSocket.constructorErrno=OsConstants.EPERM;
        final LabSocketServer failed=server;
        fault(LabProtocol.Reason.POLICY_DENIED,() -> failed.open());
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED,() -> failed.close());
        fault(LabProtocol.Reason.TRANSPORT_LOST,() -> failed.open());
        check(!failed.cleanupVerified() && LocalServerSocket.constructors==1 && Os.directCloses==0);
        clock=reset();server=new LabSocketServer(INSTANCE,clock,15100);server.open();LocalServerSocket.acceptUnknown=true;
        final LabSocketServer ambiguous=server;
        fault(LabProtocol.Reason.TRANSPORT_LOST,() -> ambiguous.accept(5100));
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED,() -> ambiguous.close());check(!ambiguous.cleanupVerified());
        clock=reset();server=new LabSocketServer(INSTANCE,clock,15100);server.open();LocalServerSocket.spuriousReady=true;
        final LabSocketServer spurious=server;
        fault(LabProtocol.Reason.DEADLINE,() -> spurious.accept(200));
        server.close();check(server.cleanupVerified() && LocalServerSocket.accepts==1);
        clock=reset();Os.ignoreAllFlagSets=true;server=new LabSocketServer(INSTANCE,clock,15100);
        final LabSocketServer flags=server;fault(LabProtocol.Reason.TRANSPORT_LOST,() -> flags.open());
        fault(LabProtocol.Reason.TRANSPORT_LOST,() -> flags.accept(5100));
        server.close();check(server.cleanupVerified() && LocalServerSocket.accepts==0);
    }
    static void framesAndDeadlines() throws Exception {
        Clock clock=reset();LocalSocket socket=LocalSocket.accepted(2000,observe());FileDescriptor fd=socket.getFileDescriptor();
        LabSocketChannel peer=channel(socket,clock);
        LabSocketChannel.FrameRead frame=peer.readFrame(2100,null);check(frame.kind==LabSocketChannel.Kind.DATA);
        check(LabProtocol.decode(frame.bytes(),new LabProtocol.InputBudget()).opcode==LabProtocol.Opcode.OBSERVE);
        byte[] copy=frame.bytes();copy[0]=99;check(frame.bytes()[0]==0);
        check(peer.readFrame(2100,null).kind==LabSocketChannel.Kind.EOF);peer.close();check(peer.cleanupVerified());
        clock=reset();socket=LocalSocket.accepted(2000,observe());fd=socket.getFileDescriptor();peer=channel(socket,clock);
        check(peer.readFrame(2100,() -> true).kind==LabSocketChannel.Kind.STOP && Os.state(fd).reads==0);
        peer.close();
        for(byte[] input:new byte[][]{new byte[]{0,0},Arrays.copyOf(observe(),9),ByteBuffer.allocate(4).putInt(Integer.MAX_VALUE).array()}) {
            clock=reset();socket=LocalSocket.accepted(2000,input);peer=channel(socket,clock);final LabSocketChannel bad=peer;
            fault(input.length==4 ? LabProtocol.Reason.LIMIT_EXCEEDED : LabProtocol.Reason.INVALID_FRAME,() -> bad.readFrame(2100,null));
            peer.close();check(peer.cleanupVerified());
        }
        clock=reset();socket=LocalSocket.accepted(2000,new byte[0]);fd=socket.getFileDescriptor();
        Os.state(fd).eof=false;peer=channel(socket,clock);final LabSocketChannel blocked=peer;
        fault(LabProtocol.Reason.DEADLINE,() -> blocked.readFrame(200,null));
        check(Os.state(fd).reads==0);peer.close();
        clock=reset();socket=LocalSocket.accepted(2000,observe());peer=channel(socket,clock);clock.time=99;
        final LabSocketChannel backwards=peer;fault(LabProtocol.Reason.DEADLINE,() -> backwards.readFrame(2100,null));peer.close();
        clock=reset();socket=LocalSocket.accepted(2000,observe());fd=socket.getFileDescriptor();peer=channel(socket,clock);
        Os.state(fd).readErrno=OsConstants.EAGAIN;check(peer.readFrame(2100,null).kind==LabSocketChannel.Kind.DATA);peer.close();
        clock=reset();byte[] seven=new byte[70];for(int i=0;i<7;i++)System.arraycopy(observe(),0,seven,i*10,10);
        socket=LocalSocket.accepted(2000,seven);fd=socket.getFileDescriptor();peer=channel(socket,clock);
        for(int i=0;i<6;i++)check(peer.readFrame(2100,null).kind==LabSocketChannel.Kind.DATA);
        final LabSocketChannel tooMany=peer;fault(LabProtocol.Reason.LIMIT_EXCEEDED,() -> tooMany.readFrame(2100,null));
        check(Os.state(fd).position==60);peer.close();
        clock=reset();byte[] trailing=ByteBuffer.allocate(11).putInt(7).putInt(LabProtocol.MAGIC).put((byte)1).put((byte)1).put((byte)0).array();
        socket=LocalSocket.accepted(2000,trailing);peer=channel(socket,clock);
        final byte[] invalid=peer.readFrame(2100,null).bytes();
        fault(LabProtocol.Reason.INVALID_FRAME,() -> LabProtocol.decode(invalid,new LabProtocol.InputBudget()));
        peer.abortIo();final LabSocketChannel poisoned=peer;
        fault(LabProtocol.Reason.TRANSPORT_LOST,() -> poisoned.writeFrame(observe(),2100));peer.close();
        clock=reset();socket=LocalSocket.accepted(2000,observe());fd=socket.getFileDescriptor();
        Os.state(fd).nullCredentials=true;peer=new LabSocketChannel(socket,clock,15100);peer.prepare();
        final LabSocketChannel unknown=peer;fault(LabProtocol.Reason.PEER_UNKNOWN,() -> unknown.admitPeer(2000));
        check(Os.state(fd).reads==0);peer.close();
        clock=reset();socket=LocalSocket.accepted(2000,observe());fd=socket.getFileDescriptor();peer=channel(socket,clock);
        Os.state(fd).readErrno=OsConstants.EACCES;final LabSocketChannel policy=peer;
        fault(LabProtocol.Reason.POLICY_DENIED,() -> policy.readFrame(2100,null));
        check(Os.state(fd).reads==1);peer.close();
        clock=reset();socket=LocalSocket.accepted(2000,new byte[0]);fd=socket.getFileDescriptor();peer=channel(socket,clock);
        Os.state(fd).eof=false;final Clock idleClock=clock;final FileDescriptor idleFd=fd;
        Os.pollHook=() -> { idleClock.time=5100;Os.state(idleFd).input=observe(); };
        check(peer.readFrame(15100,null).kind==LabSocketChannel.Kind.DATA && clock.time==5100);peer.close();
        for(byte[] slow:new byte[][]{Arrays.copyOf(observe(),1),Arrays.copyOf(observe(),5)}) {
            clock=reset();socket=LocalSocket.accepted(2000,slow);fd=socket.getFileDescriptor();peer=channel(socket,clock);
            Os.state(fd).eof=false;final LabSocketChannel slowFrame=peer;
            fault(LabProtocol.Reason.DEADLINE,() -> slowFrame.readFrame(15100,null));
            check(clock.time==2100);peer.close();
        }
        clock=reset();socket=LocalSocket.accepted(2000,new byte[0]);fd=socket.getFileDescriptor();peer=channel(socket,clock);
        Os.state(fd).eof=false;final Clock stopClock=clock;java.util.concurrent.atomic.AtomicBoolean stopped=new java.util.concurrent.atomic.AtomicBoolean();
        Os.pollHook=() -> { stopClock.time=5100;stopped.set(true); };
        check(peer.readFrame(15100,() -> stopped.get()).kind==LabSocketChannel.Kind.STOP && Os.state(fd).reads==0);peer.close();
        for(int threshold:new int[]{1,4,7,10}) {
            clock=reset();socket=LocalSocket.accepted(2000,observe());fd=socket.getFileDescriptor();peer=channel(socket,clock);
            final FileDescriptor partialFd=fd;final int consumed=threshold;final LabSocketChannel partial=peer;
            fault(LabProtocol.Reason.INVALID_FRAME,() -> partial.readFrame(15100,() -> Os.state(partialFd).position>=consumed));
            check(Os.state(fd).position>=threshold);peer.close();check(peer.cleanupVerified());
        }
        clock=reset();socket=LocalSocket.accepted(2000,observe());fd=socket.getFileDescriptor();peer=channel(socket,clock);
        Os.state(fd).readChunk=100;Os.state(fd).readAdvanceAfter=3;Os.state(fd).readAdvance=2000;
        final LabSocketChannel lateLastRead=peer;
        fault(LabProtocol.Reason.DEADLINE,() -> lateLastRead.readFrame(2100,null));peer.close();
    }
    static void writesAndCleanup() throws Exception {
        Clock clock=reset();LocalSocket socket=LocalSocket.accepted(2000,new byte[0]);FileDescriptor fd=socket.getFileDescriptor();
        LabSocketChannel peer=channel(socket,clock);Os.state(fd).writeErrno=OsConstants.EAGAIN;
        peer.writeFrame(observe(),2100);check(Arrays.equals(Os.state(fd).output.toByteArray(),observe()));
        check(Os.state(fd).writes>1);peer.close();peer.close();check(socket.closeCalls==1);
        int numeric=Os.state(fd).numeric;Os.nextNumeric=numeric;
        LocalSocket replacement=LocalSocket.accepted(2000,observe());FileDescriptor reused=replacement.getFileDescriptor();
        check(Os.state(reused).numeric==numeric);final LabSocketChannel old=peer;
        fault(LabProtocol.Reason.TRANSPORT_LOST,() -> old.readFrame(2100,null));
        fault(LabProtocol.Reason.TRANSPORT_LOST,() -> old.writeFrame(observe(),2100));
        check(Os.state(reused).reads==0 && Os.state(reused).writes==0 && Os.state(reused).closes==0);replacement.close();
        clock=reset();socket=LocalSocket.accepted(2000,observe());fd=socket.getFileDescriptor();peer=channel(socket,clock);
        Os.state(fd).writeErrno=OsConstants.EPERM;final LabSocketChannel denied=peer;
        fault(LabProtocol.Reason.POLICY_DENIED,() -> denied.writeFrame(observe(),2100));peer.close();
        clock=reset();socket=LocalSocket.accepted(2000,observe());fd=socket.getFileDescriptor();peer=channel(socket,clock);
        final Clock afterPoll=clock;Os.pollHook=() -> afterPoll.time=2100;final LabSocketChannel expired=peer;
        fault(LabProtocol.Reason.DEADLINE,() -> expired.writeFrame(observe(),2100));check(Os.state(fd).writes==0);peer.close();
        clock=reset();socket=LocalSocket.accepted(2000,observe());fd=socket.getFileDescriptor();peer=channel(socket,clock);
        Os.state(fd).writeChunk=100;Os.state(fd).writeAdvance=2000;final LabSocketChannel lateLastWrite=peer;
        fault(LabProtocol.Reason.DEADLINE,() -> lateLastWrite.writeFrame(observe(),2100));
        check(Arrays.equals(Os.state(fd).output.toByteArray(),observe()));peer.close();
        clock=reset();LabSocketServer server=new LabSocketServer(INSTANCE,clock,15100);server.open();
        Os.state(LocalServerSocket.lastAllocated).closeThrows=true;
        socket=LocalSocket.accepted(2000,observe());LocalServerSocket.queue.add(socket);final LabSocketServer badClose=server;
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED,() -> badClose.accept(5100));
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED,() -> badClose.close());
        check(socket.closeCalls==1 && LocalServerSocket.last.closeCalls==1 && !server.cleanupVerified());
        clock=reset();socket=LocalSocket.accepted(2000,observe());fd=socket.getFileDescriptor();peer=channel(socket,clock);
        Os.state(fd).closeThrows=true;final LabSocketChannel failedClose=peer;
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED,() -> failedClose.close());
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED,() -> failedClose.close());check(socket.closeCalls==1 && !peer.cleanupVerified());
        check(Os.directCloses==0);
        clock=reset();socket=LocalSocket.accepted(2000,observe());peer=channel(socket,clock);
        final LabSocketChannel interrupted=peer;Thread.currentThread().interrupt();
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED,() -> interrupted.close());
        check(Thread.currentThread().isInterrupted() && !peer.cleanupVerified());Thread.interrupted();
        check(socket.closeCalls==0);
    }
    static void client() throws Exception {
        Clock clock=reset();LabClientConnection unopened=new LabClientConnection(INSTANCE,10001,clock,15100);
        check(LocalSocket.constructors==0);unopened.close();check(unopened.cleanupVerified() && LocalSocket.lazyCloseAllocations==0);
        clock=reset();LabClientConnection client=new LabClientConnection(INSTANCE,10001,clock,15100);
        LabSocketChannel peer=client.open();check(LocalSocket.connects==1 && LocalSocket.binds==1);
        check(LocalSocket.lastBind.equals("tv.native.lab.client."+INSTANCE) && LocalSocket.lastConnect.equals("tv.native.lab."+INSTANCE));
        peer.writeFrame(observe(),2100);client.close();peer.close();client.close();check(client.cleanupVerified());
        check(LocalSocket.lazyCloseAllocations==0 && Os.directCloses==0);
        for(int errno:new int[]{OsConstants.EAGAIN,OsConstants.EINPROGRESS,OsConstants.EPERM}) {
            clock=reset();LocalSocket.connectErrno=errno;
            client=new LabClientConnection(INSTANCE,10001,clock,15100);final LabClientConnection refused=client;
            fault(errno==OsConstants.EPERM ? LabProtocol.Reason.POLICY_DENIED : LabProtocol.Reason.TRANSPORT_LOST,() -> refused.open());
            fault(LabProtocol.Reason.TRANSPORT_LOST,() -> refused.open());client.close();
            check(LocalSocket.connects==1 && LocalSocket.binds==1 && client.cleanupVerified());
        }
        clock=reset();LocalSocket.bindErrno=OsConstants.EPERM;LocalSocket.bindNoFd=true;
        client=new LabClientConnection(INSTANCE,10001,clock,15100);final LabClientConnection ambiguous=client;
        fault(LabProtocol.Reason.POLICY_DENIED,() -> ambiguous.open());
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED,() -> ambiguous.close());
        check(!client.cleanupVerified() && LocalSocket.lazyCloseAllocations==0);
        clock=reset();LocalSocket.bindErrno=OsConstants.EPERM;client=new LabClientConnection(INSTANCE,10001,clock,15100);
        final LabClientConnection allocated=client;fault(LabProtocol.Reason.POLICY_DENIED,() -> allocated.open());
        client.close();check(client.cleanupVerified() && LocalSocket.lazyCloseAllocations==0);
        final Clock valid=reset();
        fault(LabProtocol.Reason.PEER_UNKNOWN,() -> new LabClientConnection(INSTANCE,0,valid,15100));
        fault(LabProtocol.Reason.PEER_UNKNOWN,() -> new LabClientConnection(INSTANCE,2000,valid,15100));
        fault(LabProtocol.Reason.DEADLINE,() -> new LabDeadline(valid,Long.MAX_VALUE));
        fault(LabProtocol.Reason.DEADLINE,() -> new LabDeadline(valid,100));
    }
    static void closeWhilePolling() throws Exception {
        Clock clock=reset();LocalSocket socket=LocalSocket.accepted(2000,observe());
        LabSocketChannel peer=channel(socket,clock);CountDownLatch inPoll=new CountDownLatch(1),release=new CountDownLatch(1);
        Os.pollHook=() -> { inPoll.countDown();try { if(!release.await(2,TimeUnit.SECONDS))throw new AssertionError("fake poll latch"); }
            catch(InterruptedException error) { throw new AssertionError("fake poll interrupted"); } };
        final Throwable[] readerFailure=new Throwable[1];
        Thread reader=new Thread(() -> { try { peer.readFrame(2100,null); }catch(Throwable error) { readerFailure[0]=error; } });
        reader.start();check(inPoll.await(2,TimeUnit.SECONDS));
        try {
            fault(LabProtocol.Reason.CLEANUP_UNVERIFIED,() -> peer.close());
            check(!peer.cleanupVerified() && socket.closeCalls==0);
        } finally { release.countDown();reader.join(2000); }
        check(!reader.isAlive() && readerFailure[0] instanceof LabProtocol.Fault);
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED,() -> peer.close());check(socket.closeCalls==0);
    }
    public static void main(String[] args) throws Exception {
        constructorAndPeer();framesAndDeadlines();writesAndCleanup();client();closeWhilePolling();
        System.out.println("PASS public socket owner host checks: "+checks);
    }
}
