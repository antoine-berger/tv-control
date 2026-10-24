package org.tvcontrol.nativeinput;

import android.system.Os;
import android.system.OsConstants;
import java.io.IOException;

/** Conservative stop signal only at the immediate API31 native bind/connect catches. */
final class LabNativeSocketFailure {
    static LabProtocol.Fault fromBindOrConnect(IOException error) {
        LabProtocol.Fault unknown=new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
        try {
            // Structured errno remains authoritative before the JNI message fallback.
            LabProtocol.Fault structured=LabDeadline.fault(error,LabProtocol.Reason.TRANSPORT_LOST);
            if(structured.reason==LabProtocol.Reason.POLICY_DENIED)return structured;
            if(error==null || error.getClass()!=IOException.class || error.getCause()!=null)return unknown;
            String access=Os.strerror(OsConstants.EACCES),operation=Os.strerror(OsConstants.EPERM);
            if(access==null || operation==null || access.isEmpty() || operation.isEmpty())return unknown;
            // No raw message is retained, emitted, hashed or matched partially.
            if(access.equals(error.getMessage()) || operation.equals(error.getMessage()))
                return new LabProtocol.Fault(LabProtocol.Reason.POLICY_DENIED);
        } catch(Throwable ignored) {
            // Unavailable/ambiguous evidence never grants permission or permits retry.
        }
        return unknown;
    }
    private LabNativeSocketFailure(){}
}
