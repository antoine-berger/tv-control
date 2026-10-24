package org.tvcontrol.nativeinput;

import android.net.LocalSocket;
import android.system.Os;
import android.system.OsConstants;
import java.io.FileDescriptor;
import java.nio.charset.StandardCharsets;

/** Actual writer with fake syscalls; no SDK/Android/socket execution. */
public final class ClientDiagnosticTest {
    static int checks;
    static void check(boolean value){checks++;if(!value)throw new AssertionError("closed diagnostic contract");}
    static final class Clock implements IssuedEditor.Clock {long value=100;public long nowMillis(){return value;}}
    static LabClientDiagnostic.FirstFailure failure() {
        LabClientDiagnostic.FirstFailure value=new LabClientDiagnostic.FirstFailure();
        value.record(LabClientDiagnostic.Phase.CONNECT,LabProtocol.Reason.POLICY_DENIED);return value;
    }
    static void reset(){Os.reset();LocalSocket.reset();android.os.SystemClock.offset=0;Os.state(FileDescriptor.err).writeChunk=512;}
    static void noBorrowedClose() {
        check(Os.directCloses==0);check(Os.state(FileDescriptor.err).closes==0);
        check(Os.state(FileDescriptor.err).reads==0 && Os.state(FileDescriptor.err).polls==0);
    }
    static void vocabularyAndStickyCause() {
        for(LabClientDiagnostic.Phase phase:LabClientDiagnostic.Phase.values())
            for(LabProtocol.Reason reason:LabProtocol.Reason.values()) {
                LabClientDiagnostic.FirstFailure value=new LabClientDiagnostic.FirstFailure();value.record(phase,reason);
                if(reason==LabProtocol.Reason.NONE){check(value.line(true)==null);continue;}
                for(boolean clean:new boolean[]{false,true}) {
                    byte[] bytes=value.line(clean);String text=new String(bytes,StandardCharsets.US_ASCII);
                    check(bytes.length<=160);check(text.equals("TV_NATIVE_CLIENT_FAILURE_V1 phase="+phase.name()
                        +" reason="+reason.name()+" cleanup="+(clean?"VERIFIED":"UNVERIFIED")+"\n"));
                    check(text.indexOf('\n')==text.length()-1);check(!value.toString().contains(phase.name()));
                }
            }
        LabClientDiagnostic.FirstFailure value=new LabClientDiagnostic.FirstFailure();
        value.record(LabClientDiagnostic.Phase.CONNECT,new java.io.IOException("CANARY_PRIVATE_text_ref_token_path",
            new android.system.ErrnoException("CANARY_PRIVATE_error",OsConstants.EPERM)));
        value.record(LabClientDiagnostic.Phase.OUTPUT_STOP,LabProtocol.Reason.CLEANUP_UNVERIFIED);
        String line=new String(value.line(false),StandardCharsets.US_ASCII);
        check(line.equals("TV_NATIVE_CLIENT_FAILURE_V1 phase=CONNECT reason=POLICY_DENIED cleanup=UNVERIFIED\n"));
        check(!line.contains("CANARY"));
    }
    static void oneWriteOnly() throws Exception {
        reset();Clock clock=new Clock();LabDeadline original=new LabDeadline(clock,1100);
        Os.State err=Os.state(FileDescriptor.err);err.flags=16;
        LabClientDiagnostic.emit(failure(),true,original);
        check(err.writes==1 && err.fcntlGets==2 && err.fcntlSets==1);
        check(err.flags==(16|OsConstants.O_NONBLOCK));check(Os.output(FileDescriptor.err).length<=160);
        check(Os.state(FileDescriptor.in).reads==0 && Os.state(FileDescriptor.out).writes==0);noBorrowedClose();
        reset();original=new LabDeadline(new Clock(),1100);err=Os.state(FileDescriptor.err);err.writeChunk=3;
        LabClientDiagnostic.emit(failure(),false,original);check(err.writes==1);check(Os.output(FileDescriptor.err).length==3);noBorrowedClose();
        for(int code:new int[]{OsConstants.EAGAIN,OsConstants.EPERM,OsConstants.EACCES}) {
            reset();original=new LabDeadline(new Clock(),1100);err=Os.state(FileDescriptor.err);err.writeErrno=code;
            LabClientDiagnostic.emit(failure(),false,original);check(err.writes==1);check(Os.output(FileDescriptor.err).length==0);noBorrowedClose();
        }
        reset();original=new LabDeadline(new Clock(),1100);err=Os.state(FileDescriptor.err);err.fcntlErrno=OsConstants.EPERM;
        LabClientDiagnostic.emit(failure(),false,original);check(err.fcntlGets==1 && err.writes==0);noBorrowedClose();
        reset();original=new LabDeadline(new Clock(),1100);err=Os.state(FileDescriptor.err);err.ignoreFlags=true;
        LabClientDiagnostic.emit(failure(),true,original);check(err.writes==0 && err.fcntlGets==2);noBorrowedClose();
    }
    static void originalDeadline() throws Exception {
        reset();Clock clock=new Clock();LabDeadline original=new LabDeadline(clock,1100);clock.value=1100;
        LabClientDiagnostic.emit(failure(),true,original);check(Os.state(FileDescriptor.err).writes==0);check(Os.state(FileDescriptor.err).fcntlGets==0);
        reset();Clock beforeWrite=new Clock();original=new LabDeadline(beforeWrite,1100);
        Os.fcntlHook=()->{if(Os.state(FileDescriptor.err).fcntlGets==2)beforeWrite.value=1100;};
        LabClientDiagnostic.emit(failure(),true,original);check(Os.state(FileDescriptor.err).writes==0);noBorrowedClose();
        reset();clock=new Clock();original=new LabDeadline(clock,1100);clock.value=99;
        LabClientDiagnostic.emit(failure(),true,original);check(Os.state(FileDescriptor.err).writes==0);
        LabClientDiagnostic.emit(failure(),true,null);check(Os.state(FileDescriptor.err).writes==0);
    }
    static void outcomeUnchanged() {
        reset();LocalSocket.connectErrno=OsConstants.EPERM;
        int result=LabSocketClient.run(new String[]{"11111111111111111111111111111111","10001"});
        check(result==1);String line=new String(Os.output(FileDescriptor.err),StandardCharsets.US_ASCII);
        check(line.equals("TV_NATIVE_CLIENT_FAILURE_V1 phase=CONNECT reason=POLICY_DENIED cleanup=VERIFIED\n"));
        check(LocalSocket.connects==1 && LocalSocket.closes==1);check(Os.output(FileDescriptor.out).length==0);noBorrowedClose();
        reset();LocalSocket.connectErrno=OsConstants.EPERM;Os.state(FileDescriptor.err).writeErrno=OsConstants.EAGAIN;
        check(LabSocketClient.run(new String[]{"11111111111111111111111111111111","10001"})==1);
        check(Os.output(FileDescriptor.err).length==0);check(Os.state(FileDescriptor.err).writes==1);noBorrowedClose();
    }
    public static void main(String[] args) throws Exception {
        vocabularyAndStickyCause();oneWriteOnly();originalDeadline();outcomeUnchanged();
        System.out.println("PASS closed client diagnostic host checks: "+checks);
    }
}
