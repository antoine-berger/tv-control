package org.tvcontrol.nativeinput;

import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.locks.ReentrantLock;

/** One lab ingress attached to the common wrapper before resource allocation. */
final class LabSocketSession implements NativePendingRequest.Lifecycle {
    enum ReadKind { DATA, EOF, STOP }
    static final class Read {
        final ReadKind kind;
        private final byte[] bytes;
        private Read(ReadKind type,byte[] frame){kind=type;bytes=frame==null?null:frame.clone();}
        static Read data(byte[] frame) {
            if(frame==null)throw new IllegalArgumentException("missing private frame");
            return new Read(ReadKind.DATA,frame);
        }
        static Read eof(){return new Read(ReadKind.EOF,null);}
        static Read stopped(){return new Read(ReadKind.STOP,null);}
        byte[] bytes(){return bytes==null?null:bytes.clone();}
        @Override public String toString(){return "PrivateSessionRead";}
    }
    interface Port {
        void open(long deadline) throws Exception;
        void ready() throws Exception;
        void accept(long until) throws Exception;
        Read read(long until, LabSocketChannel.StopSignal stop) throws Exception;
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
    private final ReentrantLock markerBoundary = new ReentrantLock();
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
        Read received=port.read(until,signal);
        if(received==null || received.kind!=ReadKind.DATA)throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
        return LabProtocol.decode(received.bytes(),input); // Every codec fault aborts.
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
                Read received=port.read(deadline,signal);
                if(received==null)throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                if(received.kind==ReadKind.STOP && stop.get())return;
                if(received.kind!=ReadKind.DATA)throw new LabProtocol.Fault(LabProtocol.Reason.TRANSPORT_LOST);
                // DATA was already observed: cleanup cannot discard it, even if
                // stop changed at the return boundary. EOF is never idle STOP.
                LabProtocol.Command command=LabProtocol.decode(received.bytes(),input);
                if(command.opcode!=LabProtocol.Opcode.CANCEL)
                    throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
                abort(LabProtocol.Reason.CANCELLED);
                return;
            }
        } catch(Throwable error) {
            fault(error);
        } finally {
            // Exactly one marker attempt. The same boundary as stop prevents a
            // new cache write after a successful cleanup stop transition.
            boolean locked=false;
            try {
                locked=markerBoundary.tryLock(100,TimeUnit.MILLISECONDS);
                if(!locked)abort(LabProtocol.Reason.CLEANUP_UNVERIFIED);
                else if(!stop.get())store.cancel();
            } catch(Throwable ignored){abort(LabProtocol.Reason.CLEANUP_UNVERIFIED);}
            finally {if(locked)markerBoundary.unlock();watcherFinished=true;}
        }
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
        boolean stopLocked=false;
        try {
            stopLocked=markerBoundary.tryLock(100,TimeUnit.MILLISECONDS);
            if(!stopLocked)verified=false;
        } catch(InterruptedException error){Thread.currentThread().interrupt();verified=false;}
        finally {stop.set(true);if(stopLocked)markerBoundary.unlock();}
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
