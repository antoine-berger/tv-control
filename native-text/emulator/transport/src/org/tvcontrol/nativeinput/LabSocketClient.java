package org.tvcontrol.nativeinput;

import android.os.SystemClock;
import android.system.Os;
import java.io.FileDescriptor;
import java.util.Arrays;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

/** Ephemeral shell client. Payload travels only in borrowed binary stdio. */
public final class LabSocketClient {
    static final class ForwardState {
        LabProtocol.Opcode phase;
        LabReplies.Reference reference;
        boolean referenceSent;
    }
    static final class Forwarder implements Runnable {
        final LabPipeIo input;
        final LabSocketChannel channel;
        final LabDeadline deadline;
        final ForwardState state;
        final LabClientDiagnostic.FirstFailure diagnostic;
        final AtomicBoolean stop=new AtomicBoolean();
        final AtomicReference<LabProtocol.Reason> failure=new AtomicReference<LabProtocol.Reason>();
        Forwarder(LabPipeIo source,LabSocketChannel target,LabDeadline clock,ForwardState shared) {
            this(source,target,clock,shared,null);
        }
        Forwarder(LabPipeIo source,LabSocketChannel target,LabDeadline clock,ForwardState shared,
                LabClientDiagnostic.FirstFailure metadata) {
            input=source;channel=target;deadline=clock;state=shared;diagnostic=metadata;
        }
        public void run() {
            LabProtocol.InputBudget budget=new LabProtocol.InputBudget();
            try {
                while(!stop.get()) {
                    LabPipeIo.FrameRead read=input.readFrame(deadline.absolute(),()->stop.get());
                    if(read.kind==LabPipeIo.Kind.STOP)return;
                    if(read.kind==LabPipeIo.Kind.EOF)throw new LabProtocol.Fault(LabProtocol.Reason.CANCELLED);
                    byte[] bytes=read.bytes();LabProtocol.Command command=LabProtocol.decode(bytes,budget);
                    synchronized(state) {
                        if(command.opcode==LabProtocol.Opcode.OBSERVE) {
                            if(state.phase!=null)throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
                        } else if(command.opcode==LabProtocol.Opcode.COMMIT) {
                            if(state.phase!=LabProtocol.Opcode.OBSERVE || !state.referenceSent
                                    || !Arrays.equals(command.epoch(),state.reference.epoch())
                                    || !Arrays.equals(command.reference(),state.reference.reference()))
                                throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
                        } else if(state.phase==LabProtocol.Opcode.CANCEL)
                            throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
                        // A fully decoded invalid frame must not disappear when
                        // server EOF concurrently asks this watcher to stop.
                        if(stop.get())throw new LabProtocol.Fault(command.opcode==LabProtocol.Opcode.CANCEL
                                ? LabProtocol.Reason.CANCELLED : LabProtocol.Reason.TRANSPORT_LOST);
                        // State is published only after this complete write succeeds.
                        channel.writeFrame(bytes,deadline.after(2000));state.phase=command.opcode;
                    }
                }
            } catch(LabProtocol.Fault error){
                if(diagnostic!=null)diagnostic.record(LabClientDiagnostic.Phase.FORWARD_INPUT,error.reason);
                failure.compareAndSet(null,error.reason);channel.abortIo();
            } catch(Throwable error){
                if(diagnostic!=null)diagnostic.record(LabClientDiagnostic.Phase.FORWARD_INPUT,error);
                failure.compareAndSet(null,LabProtocol.Reason.TRANSPORT_LOST);channel.abortIo();
            }
        }
        boolean stopAndJoin(Thread thread) {
            boolean fenced=true;
            try {channel.stopIo();}
            catch(LabProtocol.Fault error) {
                if(diagnostic!=null)diagnostic.record(LabClientDiagnostic.Phase.STOP_JOIN,LabProtocol.Reason.CLEANUP_UNVERIFIED);
                fenced=false;failure.set(LabProtocol.Reason.CLEANUP_UNVERIFIED);
                channel.abortIo(); // Best effort only; failed fence never proves cleanup.
            } finally {stop.set(true);}
            if(thread==null)return fenced;
            try {
                thread.join(250);
                if(thread.isAlive()){thread.interrupt();thread.join(250);}
                return fenced && !thread.isAlive();
            } catch(InterruptedException error){Thread.currentThread().interrupt();return false;}
        }
    }
    static int helperUid(String value) throws LabProtocol.Fault {
        if(value==null || !value.matches("[1-9][0-9]{0,9}"))throw new LabProtocol.Fault(LabProtocol.Reason.PEER_UNKNOWN);
        try {
            int uid=Integer.parseInt(value);
            if(uid==2000)throw new LabProtocol.Fault(LabProtocol.Reason.PEER_UNKNOWN);
            return uid;
        } catch(NumberFormatException error){throw new LabProtocol.Fault(LabProtocol.Reason.PEER_UNKNOWN);}
    }
    static int run(String[] args) {
        LabClientConnection connection=null;LabPipeIo input=null,output=null;
        LabSocketChannel channel=null;Forwarder forwarder=null;Thread writer=null;
        boolean serverEof=false,clean=true;int result=1;
        LabDeadline original=null;
        LabClientDiagnostic.FirstFailure diagnostic=new LabClientDiagnostic.FirstFailure();
        LabClientDiagnostic.Phase phase=LabClientDiagnostic.Phase.ARGUMENTS;
        try {
            if(args==null || args.length!=2 || args[0]==null || !args[0].matches("[0-9a-f]{32}"))
                throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
            int helper=helperUid(args[1]);
            phase=LabClientDiagnostic.Phase.LOCAL_UID;
            if(Os.getuid()!=2000 || Os.geteuid()!=2000)throw new LabProtocol.Fault(LabProtocol.Reason.POLICY_DENIED);
            IssuedEditor.Clock clock=()->SystemClock.uptimeMillis();
            long until=Math.addExact(clock.nowMillis(),IssuedEditor.MAX_INSTANCE_MILLIS);
            LabDeadline deadline=new LabDeadline(clock,until);
            original=deadline;
            phase=LabClientDiagnostic.Phase.CONNECT;
            connection=new LabClientConnection(args[0],helper,clock,until);
            channel=connection.open(); // Mutual kernel UID admission precedes any payload read.
            phase=LabClientDiagnostic.Phase.STDIO_PREPARE;
            input=new LabPipeIo(FileDescriptor.in,clock,until);output=new LabPipeIo(FileDescriptor.out,clock,until);
            input.prepare();output.prepare();ForwardState state=new ForwardState();
            forwarder=new Forwarder(input,channel,deadline,state,diagnostic);
            phase=LabClientDiagnostic.Phase.WORKER_START;
            writer=new Thread(forwarder,"native-lab-stdin");writer.setDaemon(true);writer.start();
            for(;;) {
                phase=LabClientDiagnostic.Phase.READ_RESPONSE;
                LabSocketChannel.FrameRead read=channel.readFrame(until,null);
                if(read.kind==LabSocketChannel.Kind.EOF) {
                    synchronized(state){if(state.phase==null)throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);}
                    serverEof=true;break;
                }
                if(read.kind!=LabSocketChannel.Kind.DATA)throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                phase=LabClientDiagnostic.Phase.VALIDATE_RESPONSE;
                byte[] bytes=read.bytes();LabReplies.Reference reference=LabReplies.parseReference(bytes);
                synchronized(state) {
                    if(state.phase!=LabProtocol.Opcode.OBSERVE || state.reference!=null)
                        throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
                    if(reference.expires<=clock.nowMillis() || reference.expires>until)
                        throw new LabProtocol.Fault(LabProtocol.Reason.REF_EXPIRED);
                    state.reference=reference;output.writeFrame(bytes,deadline.after(2000));state.referenceSent=true;
                }
            }
        } catch(Throwable error) {diagnostic.record(phase,error);serverEof=false;}
        finally {
            if(forwarder!=null && !forwarder.stopAndJoin(writer)) {
                diagnostic.record(LabClientDiagnostic.Phase.STOP_JOIN,LabProtocol.Reason.CLEANUP_UNVERIFIED);clean=false;
            }
            if(input!=null)try{input.stop();}catch(LabProtocol.Fault error){diagnostic.record(LabClientDiagnostic.Phase.INPUT_STOP,error);clean=false;}
            if(connection!=null)try{connection.close();}catch(LabProtocol.Fault error){diagnostic.record(LabClientDiagnostic.Phase.SOCKET_CLOSE,error);clean=false;}
            if(connection!=null && !connection.cleanupVerified()) {
                diagnostic.record(LabClientDiagnostic.Phase.SOCKET_CLOSE,LabProtocol.Reason.CLEANUP_UNVERIFIED);clean=false;
            }
            if(input!=null && !input.cleanupVerified()) {
                diagnostic.record(LabClientDiagnostic.Phase.INPUT_STOP,LabProtocol.Reason.CLEANUP_UNVERIFIED);clean=false;
            }
            boolean noFailure=forwarder!=null && forwarder.failure.get()==null;
            if(serverEof && noFailure && clean && output!=null) {
                try {
                    IssuedEditor.Clock clock=()->SystemClock.uptimeMillis();
                    // The pipe retains the original absolute budget; this cannot extend it.
                    output.writeFrame(LabReplies.channelClosed(),Math.addExact(clock.nowMillis(),2000));result=0;
                } catch(Throwable error){diagnostic.record(LabClientDiagnostic.Phase.WRITE_CLOSED,error);result=1;}
            }
            if(output!=null)try{output.stop();}catch(LabProtocol.Fault error){diagnostic.record(LabClientDiagnostic.Phase.OUTPUT_STOP,error);clean=false;}
            if(output!=null && !output.cleanupVerified()) {
                diagnostic.record(LabClientDiagnostic.Phase.OUTPUT_STOP,LabProtocol.Reason.CLEANUP_UNVERIFIED);clean=false;
            }
            if(!clean)result=2;
            if(result!=0)LabClientDiagnostic.emit(diagnostic,clean,original);
        }
        return result;
    }
    public static void main(String[] args){System.exit(run(args));}
    private LabSocketClient(){}
}
