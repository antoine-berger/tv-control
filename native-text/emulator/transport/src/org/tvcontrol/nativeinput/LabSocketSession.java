package org.tvcontrol.nativeinput;

import java.util.concurrent.atomic.AtomicBoolean;

/** One lab ingress attached to the common wrapper before resource allocation. */
final class LabSocketSession implements NativePendingRequest.Lifecycle {
    interface Port {
        void open(long deadline) throws Exception;
        void ready() throws Exception;
        void accept(long until) throws Exception;
        byte[] read(long until, LabSocketChannel.StopSignal stop) throws Exception; // null = EOF/STOP
        void write(byte[] frame, long until) throws Exception;
        void close() throws Exception;
        boolean cleanupVerified();
    }
    interface Store {
        String open(IssuedEditor.PendingText request) throws Exception;
        void cancel() throws Exception;
        void close() throws Exception;
        boolean cleanupVerified();
    }
    interface Capture {
        Observed capture() throws Exception;
    }
    static final class Observed {
        final EditorBinding binding;
        final long at;
        Observed(EditorBinding value, long time) { binding=value; at=time; }
    }
    private final NativePendingRequest pending;
    private final IssuedEditor.Clock clock;
    private final IssuedEditor.Entropy entropy;
    private final Port port;
    private final Store store;
    private final Capture capture;
    private final AtomicBoolean stop = new AtomicBoolean();
    private final AtomicBoolean closeClaimed = new AtomicBoolean();
    private final LabProtocol.InputBudget input = new LabProtocol.InputBudget();
    private IssuedEditor core;
    private Thread watcher;
    private boolean watcherStartAttempted, watcherStarted;
    private volatile boolean watcherFinished, closed, clean;
    private long deadline;
    private LabProtocol.Reason earlyFailure;
    private IssuedEditor.Outcome outcome;

    LabSocketSession(NativePendingRequest owner, IssuedEditor.Clock time, IssuedEditor.Entropy random,
            Port channel, Store cache, Capture issuer) {
        pending=owner; clock=time; entropy=random; port=channel; store=cache; capture=issuer;
        // No clock, FD, filesystem, thread or Android calls in this constructor.
    }
    private long until(int millis) throws LabProtocol.Fault {
        long now=clock.nowMillis();
        if(now<0 || now>=deadline)throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);
        try {return Math.min(deadline,Math.addExact(now,millis));}
        catch(ArithmeticException error){throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);}
    }
    private void abort(LabProtocol.Reason reason) {
        if(core!=null)core.abort(reason);
        else if(earlyFailure==null)earlyFailure=reason;
    }
    private void fault(Throwable error) {
        abort(error instanceof LabProtocol.Fault ? ((LabProtocol.Fault)error).reason
            : LabProtocol.Reason.TRANSPORT_LOST);
    }
    private LabProtocol.Command read(long until, LabSocketChannel.StopSignal signal) throws Exception {
        byte[] frame=port.read(until,signal);
        if(frame==null)throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
        return LabProtocol.decode(frame,input); // Every codec fault is caught by ingress/watcher and aborts.
    }
    boolean prepare() {
        try {
            deadline=Math.addExact(pending.startedUptime(),IssuedEditor.MAX_INSTANCE_MILLIS);
            core=new IssuedEditor(clock,entropy,deadline);
            port.open(deadline);
            port.ready(); // No consuming probe: host sees READY only after owned listener is ready.
            port.accept(until(5000)); // Port performs shell UID admission before returning.
            LabProtocol.Command first=read(until(2000),null);
            if(first.opcode==LabProtocol.Opcode.CANCEL){abort(LabProtocol.Reason.CANCELLED);return false;}
            if(first.opcode!=LabProtocol.Opcode.OBSERVE)throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
            Observed observed=capture.capture();
            IssuedEditor.Lease lease=core.issue(observed.binding,observed.at);
            port.write(LabReplies.reference(lease),until(2000));
            LabProtocol.Command commit=read(Math.min(lease.expires,until(2000)),null);
            if(commit.opcode==LabProtocol.Opcode.CANCEL){abort(LabProtocol.Reason.CANCELLED);return false;}
            IssuedEditor.PendingText text=core.consume(commit);
            pending.token(store.open(text)); // Cache uses immutable original expiry, never now+TTL.
            watcher=new Thread(new Runnable(){public void run(){watch();}},"native-lab-cancel");
            watcherStartAttempted=true;
            watcher.start();
            watcherStarted=true;
            return true;
        } catch(Throwable error) {fault(error);return false;}
    }
    private void watch() {
        try {
            LabSocketChannel.StopSignal signal=new LabSocketChannel.StopSignal(){public boolean stopped(){return stop.get();}};
            while(!stop.get()) {
                byte[] frame=port.read(deadline,signal);
                if(stop.get())return;
                if(frame==null)throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                LabProtocol.Command command=LabProtocol.decode(frame,input);
                if(command.opcode!=LabProtocol.Opcode.CANCEL)
                    throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
                abort(LabProtocol.Reason.CANCELLED);
                store.cancel();
                return;
            }
        } catch(Throwable error) {
            fault(error);
            try {store.cancel();}catch(Throwable ignored){abort(LabProtocol.Reason.CLEANUP_UNVERIFIED);}
        } finally {watcherFinished=true;}
    }
    @Override public boolean beforeEntry() {
        try {core.markEngineEntered();return true;}
        catch(Throwable error){fault(error);return false;}
    }
    @Override public void result(String status) throws Exception {core.engineResult(status);}
    @Override public void failed(){abort(LabProtocol.Reason.TRANSPORT_LOST);}
    @Override public void close() throws Exception {
        if(!closeClaimed.compareAndSet(false,true))throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
        boolean verified=true;
        if(core!=null)try {core.beginCleanup();}catch(Throwable error){verified=false;}
        stop.set(true);
        if(watcherStartAttempted) {
            if(!watcherStarted)verified=false;
            if(watcher!=null)try {watcher.join(300);}catch(InterruptedException error){Thread.currentThread().interrupt();verified=false;}
            if(watcher==null || watcher.isAlive() || !watcherFinished)verified=false;
        }
        try {port.close();}catch(Throwable error){verified=false;}
        try {if(!port.cleanupVerified())verified=false;}catch(Throwable error){verified=false;}
        try {store.close();}catch(Throwable error){verified=false;}
        try {if(!store.cleanupVerified())verified=false;}catch(Throwable error){verified=false;}
        clean=verified;
        if(core!=null)try {outcome=core.finishCleanup(verified);}catch(Throwable error){clean=false;}
        closed=true;
        if(!clean)throw new LabProtocol.Fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
    }
    @Override public boolean cleanupVerified(){return closed && clean;}
    @Override public String terminalStatus() {
        if(outcome!=null)return outcome.status.name().toLowerCase(java.util.Locale.ROOT);
        return clean && earlyFailure!=null ? "refused" : "uncertain";
    }
    @Override public String terminalReason() {
        if(outcome!=null)return outcome.reason.name();
        return (earlyFailure!=null ? earlyFailure : LabProtocol.Reason.CLEANUP_UNVERIFIED).name();
    }
    @Override public String toString(){return "OwnedLabSocketSession";}
}
