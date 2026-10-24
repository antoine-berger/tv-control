package org.tvcontrol.nativeinput;

import android.net.LocalSocket;
import android.system.Os;
import android.system.OsConstants;
import java.io.IOException;

/** Host context/negative checks only, not Android permission or SELinux qualification. */
public final class NativeSocketFailureTest {
    static int checks;
    static void check(boolean value){checks++;if(!value)throw new AssertionError("contextual errno contract");}
    static void reset(){
        SocketIoTest.reset();Os.accessText="Permission denied";Os.operationText="Operation not permitted";
        Os.strerrorCalls=0;Os.strerrorThrows=false;LocalSocket.bindIoMessage=null;LocalSocket.connectIoMessage=null;
    }
    static void reason(IOException error,LabProtocol.Reason expected) {
        LabProtocol.Fault result=LabNativeSocketFailure.fromBindOrConnect(error);
        check(result.reason==expected);check(result.getMessage().equals(expected.name()));check(result.getCause()==null);
    }
    static final class GuardedMessage extends IOException {
        int reads;
        GuardedMessage(Throwable cause){super("CANARY_PRIVATE_reference_payload",cause);}
        @Override public String getMessage(){reads++;throw new AssertionError("must not inspect subclass message");}
    }
    static void directAndStructured() {
        reset();reason(new IOException(Os.accessText),LabProtocol.Reason.POLICY_DENIED);check(Os.strerrorCalls==2);
        reset();reason(new IOException(Os.operationText),LabProtocol.Reason.POLICY_DENIED);check(Os.strerrorCalls==2);
        for(int code:new int[]{OsConstants.EACCES,OsConstants.EPERM,OsConstants.EAGAIN}) {
            reset();IOException wrapped=new IOException(Os.accessText,new android.system.ErrnoException("canary",code));
            reason(wrapped,code==OsConstants.EAGAIN?LabProtocol.Reason.TRANSPORT_LOST:LabProtocol.Reason.POLICY_DENIED);
            check(Os.strerrorCalls==0);
        }
        reset();GuardedMessage subclass=new GuardedMessage(null);reason(subclass,LabProtocol.Reason.TRANSPORT_LOST);
        check(subclass.reads==0 && Os.strerrorCalls==0);
        reset();subclass=new GuardedMessage(new android.system.ErrnoException("canary",OsConstants.EPERM));
        reason(subclass,LabProtocol.Reason.POLICY_DENIED);check(subclass.reads==0 && Os.strerrorCalls==0);
    }
    static void unknownNeverGrants() {
        for(String message:new String[]{null,"","permission denied"," Permission denied","Permission denied\n",
                "prefix Permission denied","Operation not permitted suffix","CANARY_PRIVATE_opaque_text"}) {
            reset();reason(new IOException(message),LabProtocol.Reason.TRANSPORT_LOST);
        }
        reset();reason(null,LabProtocol.Reason.TRANSPORT_LOST);check(Os.strerrorCalls==0);
        reset();Os.accessText=null;reason(new IOException("Permission denied"),LabProtocol.Reason.TRANSPORT_LOST);
        reset();Os.operationText="";reason(new IOException("Permission denied"),LabProtocol.Reason.TRANSPORT_LOST);
        reset();Os.strerrorThrows=true;reason(new IOException("Permission denied"),LabProtocol.Reason.TRANSPORT_LOST);
        reset();Os.accessText="locale-specific-public-constant";Os.operationText="other-public-constant";
        reason(new IOException("Permission denied"),LabProtocol.Reason.TRANSPORT_LOST);
    }
    static void onlyImmediateBindConnect() throws Exception {
        for(boolean bind:new boolean[]{true,false}) {
            reset();if(bind)LocalSocket.bindIoMessage=Os.accessText;else LocalSocket.connectIoMessage=Os.operationText;
            SocketIoTest.Clock clock=new SocketIoTest.Clock();
            LabClientConnection connection=new LabClientConnection(SocketIoTest.INSTANCE,10001,clock,15100);
            SocketIoTest.fault(LabProtocol.Reason.POLICY_DENIED,()->connection.open());
            check(LocalSocket.binds==1 && LocalSocket.connects==(bind?0:1));check(Os.strerrorCalls==2);
            connection.close();check(connection.cleanupVerified());
        }
        reset();check(LabDeadline.fault(new IOException(Os.accessText),LabProtocol.Reason.TRANSPORT_LOST).reason==LabProtocol.Reason.TRANSPORT_LOST);
        check(Os.strerrorCalls==0); // Generic helpers must never parse messages.
        reset();SocketIoTest.Clock clock=new SocketIoTest.Clock();
        LocalSocket socket=LocalSocket.accepted(2000,SocketIoTest.observe());
        java.io.FileDescriptor fd=socket.getFileDescriptor();LabSocketChannel channel=SocketIoTest.channel(socket,clock);
        Os.state(fd).readIoMessage=Os.accessText;
        SocketIoTest.fault(LabProtocol.Reason.TRANSPORT_LOST,()->channel.readFrame(2100,null));
        check(Os.strerrorCalls==0);channel.close();check(channel.cleanupVerified());
        reset();clock=new SocketIoTest.Clock();socket=LocalSocket.accepted(2000,new byte[0]);fd=socket.getFileDescriptor();
        final LabSocketChannel closing=SocketIoTest.channel(socket,clock);Os.state(fd).closeThrows=true;Os.state(fd).closeIoMessage=Os.accessText;
        SocketIoTest.fault(LabProtocol.Reason.CLEANUP_UNVERIFIED,()->closing.close());check(Os.strerrorCalls==0);check(!closing.cleanupVerified());
    }
    public static void main(String[] args) throws Exception {
        directAndStructured();unknownNeverGrants();onlyImmediateBindConnect();
        System.out.println("PASS contextual native socket failure host checks: "+checks);
    }
}
