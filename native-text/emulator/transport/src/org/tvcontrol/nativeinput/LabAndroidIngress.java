package org.tvcontrol.nativeinput;

import android.app.Instrumentation;
import android.app.UiAutomation;
import android.os.Bundle;
import android.os.SystemClock;
import java.io.File;
import java.security.SecureRandom;

/** Android adapters for the owned synthetic lab fixture only, never a production target. */
final class LabAndroidIngress {
    // Closed test-fixture scope: no arbitrary foreground app is admitted by the lab transport.
    static final String FIXTURE_PACKAGE="org.tvcontrol.fixture";
    static final String FIXTURE_RESOURCE="org.tvcontrol.fixture:id/editor";
    private static final class Port implements LabSocketSession.Port {
        final String instance;
        final IssuedEditor.Clock clock;
        final Instrumentation instrumentation;
        LabSocketServer server;
        LabSocketChannel channel;
        long deadline;
        boolean openAttempted,closed;
        Port(String id,IssuedEditor.Clock time,Instrumentation wrapper){instance=id;clock=time;instrumentation=wrapper;}
        public void open(long until) throws Exception {
            if(openAttempted)throw new LabProtocol.Fault(LabProtocol.Reason.REF_CONSUMED);
            openAttempted=true;deadline=until;
            // The server constructor performs no FD allocation; owner is assigned before open.
            server=new LabSocketServer(instance,clock,until);
            server.open();
        }
        public void ready() throws Exception {
            if(server==null || closed)throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
            Bundle status=new Bundle();
            status.putString("tv_native_lab_ready_v1","READY");
            status.putString("tv_native_lab_protocol_v1","1");
            status.putString("tv_native_lab_instance_v1",instance);
            instrumentation.sendStatus(1,status);
        }
        public void accept(long until) throws Exception {
            channel=server.accept(until);
            channel.admitPeer(2000); // Standard Android shell UID, before any private frame.
        }
        public LabSocketSession.Read read(long until,LabSocketChannel.StopSignal stop) throws Exception {
            LabSocketChannel.FrameRead result=channel.readFrame(until,stop);
            if(result.kind==LabSocketChannel.Kind.DATA)return LabSocketSession.Read.data(result.bytes());
            return result.kind==LabSocketChannel.Kind.EOF ? LabSocketSession.Read.eof() : LabSocketSession.Read.stopped();
        }
        public void write(byte[] frame,long until) throws Exception {channel.writeFrame(frame,until);}
        public void close() throws Exception {closed=true;if(server!=null)server.close();}
        public boolean cleanupVerified(){return closed && (server==null || server.cleanupVerified());}
    }
    private static final class Store implements LabSocketSession.Store {
        final LabPrivateRequest scratch;
        Store(File contextCache){scratch=new LabPrivateRequest(contextCache);}
        public String open(IssuedEditor.PendingText request) throws Exception {scratch.open(request);return scratch.token();}
        public void cancel() throws Exception {scratch.cancel();}
        public void close() throws Exception {scratch.close();}
        public boolean cleanupVerified(){return scratch.cleanupVerified();}
    }
    static LabSocketSession create(final UiAutomation automation,NativePendingRequest owner,
            Instrumentation instrumentation,String instance) {
        final IssuedEditor.Clock clock=new IssuedEditor.Clock(){public long nowMillis(){return SystemClock.uptimeMillis();}};
        IssuedEditor.Entropy entropy=new IssuedEditor.Entropy(){
            private SecureRandom random;
            public void fill(byte[] bytes){if(random==null)random=new SecureRandom();random.nextBytes(bytes);}
        };
        return new LabSocketSession(owner,clock,entropy,new Port(instance,clock,instrumentation),new Store(owner.cache()),
            new LabSocketSession.Capture(){public LabSocketSession.Observed capture() throws Exception {
                LabEditorIssuer.Capture observed=LabEditorIssuer.capture(automation,clock,FIXTURE_PACKAGE,FIXTURE_RESOURCE);
                return new LabSocketSession.Observed(observed.binding,observed.observedAt);
            }});
    }
    private LabAndroidIngress(){}
}
