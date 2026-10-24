package org.tvcontrol.nativeinput;

import android.system.Os;
import android.system.OsConstants;
import java.io.FileDescriptor;
import java.nio.charset.StandardCharsets;

/** Failure-only metadata. Never accepts a message, payload, identifier or raw exception string. */
final class LabClientDiagnostic {
    enum Phase { ARGUMENTS, LOCAL_UID, CONNECT, STDIO_PREPARE, WORKER_START,
        READ_RESPONSE, VALIDATE_RESPONSE, FORWARD_INPUT, STOP_JOIN, INPUT_STOP,
        SOCKET_CLOSE, WRITE_CLOSED, OUTPUT_STOP }
    static final class FirstFailure {
        private Phase phase;
        private LabProtocol.Reason reason;
        synchronized void record(Phase where,LabProtocol.Reason why) {
            if(phase==null && where!=null && why!=null && why!=LabProtocol.Reason.NONE) {
                phase=where;reason=why;
            }
        }
        void record(Phase where,Throwable error) {
            record(where,error instanceof LabProtocol.Fault ? ((LabProtocol.Fault)error).reason
                : LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST).reason);
        }
        synchronized byte[] line(boolean clean) {
            if(phase==null || reason==null || reason==LabProtocol.Reason.NONE)return null;
            String value="TV_NATIVE_CLIENT_FAILURE_V1 phase="+phase.name()+" reason="+reason.name()
                +" cleanup="+(clean?"VERIFIED":"UNVERIFIED")+"\n";
            byte[] bytes=value.getBytes(StandardCharsets.US_ASCII);
            return bytes.length<=160 ? bytes : null;
        }
        @Override public String toString(){return "ClosedClientFailure";}
    }
    static void emit(FirstFailure failure,boolean clean,LabDeadline original) {
        if(failure==null || original==null)return;
        try {
            byte[] bytes=failure.line(clean);
            if(bytes==null)return;
            original.remaining(original.absolute(),1);
            FileDescriptor borrowed=FileDescriptor.err;
            int flags=Os.fcntlInt(borrowed,OsConstants.F_GETFL,0);
            Os.fcntlInt(borrowed,OsConstants.F_SETFL,flags|OsConstants.O_NONBLOCK);
            if((Os.fcntlInt(borrowed,OsConstants.F_GETFL,0)&OsConstants.O_NONBLOCK)==0)return;
            original.remaining(original.absolute(),1);
            // Exactly one nonblocking write: partial/EAGAIN is deliberately not retried.
            Os.write(borrowed,bytes,0,bytes.length);
        } catch(Throwable ignored) {
            // Best effort only. Never alter operation verdict or cleanup proof.
        }
        // stderr is borrowed: no close, replacement, poll, thread or new deadline.
    }
    private LabClientDiagnostic(){}
}
