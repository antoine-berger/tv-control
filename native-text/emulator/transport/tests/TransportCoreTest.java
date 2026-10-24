package org.tvcontrol.nativeinput;

import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.concurrent.atomic.AtomicInteger;

/** Actual pure transport classes only; no Android, SDK, sockets or app processes. */
public final class TransportCoreTest {
    static int checks;
    interface Checked { void run() throws Exception; }
    static void check(boolean value) { checks++; if (!value) throw new AssertionError("transport core invariant"); }
    static void fault(LabProtocol.Reason expected, Checked action) throws Exception {
        try { action.run(); } catch (LabProtocol.Fault failure) {
            check(failure.reason == expected); check(failure.getMessage().equals(expected.name())); return;
        }
        throw new AssertionError("expected fixed transport refusal");
    }
    static byte[] frame(int operation, byte[] epoch, byte[] reference, byte[] text) throws Exception {
        ByteArrayOutputStream body = new ByteArrayOutputStream();
        DataOutputStream out = new DataOutputStream(body);
        out.writeInt(LabProtocol.MAGIC); out.writeByte(1); out.writeByte(operation);
        if (operation == 2) { out.write(epoch); out.write(reference); out.writeInt(text.length); out.write(text); }
        ByteArrayOutputStream framed = new ByteArrayOutputStream();
        out = new DataOutputStream(framed); out.writeInt(body.size()); out.write(body.toByteArray());
        return framed.toByteArray();
    }
    static LabProtocol.Command commit(IssuedEditor.Lease lease, String text) throws Exception {
        return LabProtocol.decode(frame(2, lease.epoch(), lease.reference(), text.getBytes(StandardCharsets.UTF_8)), new LabProtocol.InputBudget());
    }
    static EditorBinding binding(String text) throws Exception {
        return new EditorBinding("fixture", "fixture:id/editor", "android.widget.EditText", 7,
            1, 2, 100, 40, text, text == null ? 0 : text.length(), text == null ? 0 : text.length());
    }
    static final class Clock implements IssuedEditor.Clock {
        long time=100;
        public long nowMillis() { return time; }
    }
    static IssuedEditor operation(Clock clock, int seed, long deadline) throws Exception {
        return new IssuedEditor(clock, new IssuedEditor.Entropy() {
            int next=seed;
            public void fill(byte[] value) { Arrays.fill(value, (byte)next++); }
        }, deadline);
    }
    static IssuedEditor.Outcome finish(IssuedEditor operation, boolean clean) throws Exception {
        operation.beginCleanup(); return operation.finishCleanup(clean);
    }
    static IssuedEditor entered(Clock clock) throws Exception {
        IssuedEditor operation=operation(clock, 1, 15100);
        IssuedEditor.Lease ref=operation.issue(binding("seed"), clock.time);
        operation.consume(commit(ref, " é🙂%s'")); operation.markEngineEntered(); return operation;
    }
    static void codec() throws Exception {
        byte[] nonce=new byte[16];
        byte[] valid=frame(2, nonce, nonce, " é🙂%s'".getBytes(StandardCharsets.UTF_8));
        LabProtocol.Command decoded=LabProtocol.decode(valid, new LabProtocol.InputBudget());
        check(decoded.opcode==LabProtocol.Opcode.COMMIT); check(decoded.append().equals(" é🙂%s'"));
        check(!decoded.toString().contains(decoded.append()));
        byte[] changed=decoded.reference(); changed[0]=1; check(decoded.reference()[0]==0);
        for (int n=0;n<valid.length;n++) {
            final byte[] shortFrame=Arrays.copyOf(valid,n);
            fault(LabProtocol.Reason.INVALID_FRAME, () -> LabProtocol.decode(shortFrame,new LabProtocol.InputBudget()));
        }
        byte[] trailing=Arrays.copyOf(valid,valid.length+1);
        fault(LabProtocol.Reason.INVALID_FRAME, () -> LabProtocol.decode(trailing,new LabProtocol.InputBudget()));
        for (int offset:new int[]{4,8,9}) {
            byte[] bad=valid.clone(); bad[offset]=(byte)255;
            fault(LabProtocol.Reason.INVALID_FRAME, () -> LabProtocol.decode(bad,new LabProtocol.InputBudget()));
        }
        for (byte[] invalid:new byte[][]{{(byte)0xc0,(byte)0x80},{(byte)0xff},{(byte)0xed,(byte)0xa0,(byte)0x80}}) {
            fault(LabProtocol.Reason.INVALID_FRAME, () -> LabProtocol.decode(frame(2,nonce,nonce,invalid),new LabProtocol.InputBudget()));
        }
        fault(LabProtocol.Reason.LIMIT_EXCEEDED, () -> LabProtocol.decode(frame(2,nonce,nonce,new byte[0]),new LabProtocol.InputBudget()));
        fault(LabProtocol.Reason.LIMIT_EXCEEDED, () -> LabProtocol.decode(frame(2,nonce,nonce,new byte[2049]),new LabProtocol.InputBudget()));
        char[] many=new char[257]; Arrays.fill(many,'a');
        fault(LabProtocol.Reason.LIMIT_EXCEEDED, () -> LabProtocol.decode(frame(2,nonce,nonce,new String(many).getBytes(StandardCharsets.UTF_8)),new LabProtocol.InputBudget()));
        fault(LabProtocol.Reason.INVALID_FRAME, () -> LabProtocol.checkedBodyLength(-1));
        fault(LabProtocol.Reason.LIMIT_EXCEEDED, () -> LabProtocol.checkedBodyLength(Integer.MAX_VALUE));
        check(LabProtocol.checkedBodyLength(16380)==16380);
        LabProtocol.InputBudget budget=new LabProtocol.InputBudget();
        for(int i=0;i<6;i++) LabProtocol.decode(frame(1,null,null,null),budget);
        fault(LabProtocol.Reason.LIMIT_EXCEEDED, () -> LabProtocol.decode(frame(1,null,null,null),budget));
        LabProtocol.InputBudget bytes=new LabProtocol.InputBudget(); bytes.charge(16384); bytes.charge(16384);
        fault(LabProtocol.Reason.LIMIT_EXCEEDED, () -> bytes.charge(1));
        fault(LabProtocol.Reason.LIMIT_EXCEEDED, () -> new LabProtocol.InputBudget().charge(-1));
    }
    static void references() throws Exception {
        Clock clock=new Clock(); IssuedEditor operation=operation(clock,1,15100);
        IssuedEditor.Lease lease=operation.issue(binding("seed"),100);
        check(lease.expires==4100);
        lease.epoch()[0]=99; lease.reference()[0]=99;
        clock.time=4099;
        IssuedEditor.PendingText pending=operation.consume(commit(lease," é🙂%s'"));
        check(pending.expires==4100); check(pending.after().equals("seed é🙂%s'"));
        check(!pending.toString().contains(pending.after()));
        operation.markEngineEntered(); operation.engineResult("verified");
        fault(LabProtocol.Reason.TRANSPORT_LOST, () -> operation.terminal());
        IssuedEditor.Outcome result=finish(operation,true);
        check(result.status==LabProtocol.Status.VERIFIED && result.reason==LabProtocol.Reason.NONE);
        check(result.engineEntered && result.cleanupVerified);
        fault(LabProtocol.Reason.REF_CONSUMED, () -> operation.consume(commit(lease,"x")));
        check(operation.terminal().status==LabProtocol.Status.VERIFIED);
        Clock shortClock=new Clock(); IssuedEditor shortOp=operation(shortClock,5,200);
        IssuedEditor.Lease shortLease=shortOp.issue(binding("seed"),100);
        check(shortLease.expires==200); shortClock.time=199;
        check(shortOp.consume(commit(shortLease,"x")).expires==200);
        Clock expiry=new Clock(); IssuedEditor expired=operation(expiry,1,15100);
        IssuedEditor.Lease old=expired.issue(binding("seed"),100); expiry.time=4100;
        fault(LabProtocol.Reason.REF_EXPIRED, () -> expired.consume(commit(old,"x")));
        check(!finish(expired,true).engineEntered);
        Clock enterClock=new Clock(); IssuedEditor late=operation(enterClock,1,15100);
        IssuedEditor.Lease ref=late.issue(binding("seed"),100); late.consume(commit(ref,"x")); enterClock.time=4100;
        fault(LabProtocol.Reason.REF_EXPIRED, () -> late.markEngineEntered());
        check(!finish(late,true).engineEntered);
        Clock backwards=new Clock(); IssuedEditor back=operation(backwards,1,15100);
        IssuedEditor.Lease b=back.issue(binding("seed"),100); backwards.time=99;
        fault(LabProtocol.Reason.DEADLINE, () -> back.consume(commit(b,"x")));
        Clock aClock=new Clock(), bClock=new Clock();
        IssuedEditor a=operation(aClock,20,15100), other=operation(bClock,40,15100);
        IssuedEditor.Lease aRef=a.issue(binding("seed"),100), bRef=other.issue(binding("seed"),100);
        fault(LabProtocol.Reason.REF_UNKNOWN, () -> a.consume(commit(bRef,"x")));
        check(other.consume(commit(bRef,"x")).expires==4100);
        check(a.phase()==IssuedEditor.Phase.ABORTED && other.phase()==IssuedEditor.Phase.CONSUMED);
        fault(LabProtocol.Reason.DEADLINE, () -> operation(new Clock(),1,Long.MAX_VALUE));
        Clock over=new Clock(); over.time=Long.MAX_VALUE-10;
        IssuedEditor overflow=operation(over,1,Long.MAX_VALUE);
        fault(LabProtocol.Reason.DEADLINE, () -> overflow.issue(binding("seed"),over.time));
        Clock staleClock=new Clock(); IssuedEditor stale=operation(staleClock,1,15100); staleClock.time=4100;
        fault(LabProtocol.Reason.REF_EXPIRED, () -> stale.issue(binding("seed"),100));
        Clock futureClock=new Clock(); IssuedEditor future=operation(futureClock,1,15100);
        fault(LabProtocol.Reason.EDITOR_UNSUPPORTED, () -> future.issue(binding("seed"),101));
        final int[] randomCalls={0}; Clock brokenClock=new Clock();
        IssuedEditor broken=new IssuedEditor(brokenClock, bytes -> {
            if(++randomCalls[0]==2) throw new IllegalStateException("fixture-private-value");
        },15100);
        fault(LabProtocol.Reason.TRANSPORT_LOST, () -> broken.issue(binding("seed"),100));
        fault(LabProtocol.Reason.REF_CONSUMED, () -> broken.issue(binding("seed"),100));
    }
    static void effects() throws Exception {
        for (LabProtocol.Reason reason:new LabProtocol.Reason[]{LabProtocol.Reason.CANCELLED,
                LabProtocol.Reason.INVALID_FRAME,LabProtocol.Reason.TRANSPORT_LOST,LabProtocol.Reason.DEADLINE}) {
            Clock clock=new Clock(); IssuedEditor operation=entered(clock);
            operation.abort(reason); operation.engineResult("verified");
            IssuedEditor.Outcome result=finish(operation,true);
            check(result.status==LabProtocol.Status.UNCERTAIN && result.reason==reason);
        }
        for (String status:new String[]{"verified","refused","uncertain","fixture-private-value",null}) {
            Clock clock=new Clock(); IssuedEditor operation=entered(clock); operation.engineResult(status);
            IssuedEditor.Outcome result=finish(operation,true);
            check(result.status==("verified".equals(status) ? LabProtocol.Status.VERIFIED
                : "refused".equals(status) ? LabProtocol.Status.REFUSED : LabProtocol.Status.UNCERTAIN));
            check(!result.toString().contains("fixture-private-value"));
        }
        Clock clock=new Clock(); IssuedEditor duplicate=entered(clock); duplicate.engineResult("verified");
        fault(LabProtocol.Reason.ENGINE_UNCERTAIN, () -> duplicate.engineResult("verified"));
        check(finish(duplicate,true).status==LabProtocol.Status.UNCERTAIN);
        IssuedEditor lost=entered(new Clock()); check(finish(lost,true).status==LabProtocol.Status.UNCERTAIN);
        IssuedEditor dirty=entered(new Clock()); dirty.engineResult("verified");
        IssuedEditor.Outcome bad=finish(dirty,false);
        check(bad.status==LabProtocol.Status.UNCERTAIN && !bad.cleanupVerified);
        Clock past=new Clock(); IssuedEditor total=entered(past); total.engineResult("verified"); past.time=15100;
        check(finish(total,true).status==LabProtocol.Status.UNCERTAIN);
        for (boolean consumed:new boolean[]{false,true}) {
            Clock c=new Clock(); IssuedEditor pre=operation(c,1,15100);
            IssuedEditor.Lease ref=pre.issue(binding("seed"),100);
            if(consumed) pre.consume(commit(ref,"x")); pre.abort(LabProtocol.Reason.CANCELLED);
            fault(LabProtocol.Reason.REF_CONSUMED, () -> pre.markEngineEntered());
            IssuedEditor.Outcome result=finish(pre,true);
            check(result.status==LabProtocol.Status.REFUSED && !result.engineEntered);
        }
        IssuedEditor lateDuplicate=entered(new Clock());
        fault(LabProtocol.Reason.REF_CONSUMED, () -> lateDuplicate.markEngineEntered());
        lateDuplicate.engineResult("verified");
        check(finish(lateDuplicate,true).status==LabProtocol.Status.UNCERTAIN);
        IssuedEditor cleanupLost=entered(new Clock()); cleanupLost.engineResult("verified"); cleanupLost.beginCleanup();
        cleanupLost.abort(LabProtocol.Reason.TRANSPORT_LOST);
        check(cleanupLost.finishCleanup(true).status==LabProtocol.Status.UNCERTAIN);
    }
    static void admissionAndRace() throws Exception {
        fault(LabProtocol.Reason.EDITOR_UNSUPPORTED, () -> binding(null));
        fault(LabProtocol.Reason.EDITOR_UNSUPPORTED, () -> binding("\ud800"));
        check(binding("").before().isEmpty()); // observable empty != opaque null
        char[] chars=new char[256]; Arrays.fill(chars,'a');
        EditorBinding full=binding(new String(chars));
        fault(LabProtocol.Reason.LIMIT_EXCEEDED, () -> full.append("x"));
        fault(LabProtocol.Reason.EDITOR_UNSUPPORTED, () -> new EditorBinding("bad\npackage","id","role",1,0,0,1,1,"seed",4,4));
        fault(LabProtocol.Reason.EDITOR_UNSUPPORTED, () -> new EditorBinding("pkg","id","role",1,0,0,1,1,"seed",1,1));
        for (int window:new int[]{-1,Integer.MIN_VALUE})
            fault(LabProtocol.Reason.EDITOR_UNSUPPORTED, () -> new EditorBinding("pkg","id","role",window,0,0,1,1,"seed",4,4));
        for (int window:new int[]{0,Integer.MAX_VALUE})
            check(new EditorBinding("pkg","id","role",window,0,0,1,1,"seed",4,4).window==window);
        Clock clock=new Clock(); IssuedEditor operation=operation(clock,1,15100);
        IssuedEditor.Lease lease=operation.issue(binding("seed"),100);
        LabProtocol.Command command=commit(lease,"x"); AtomicInteger won=new AtomicInteger(), refused=new AtomicInteger();
        Runnable consume=() -> { try { operation.consume(command); won.incrementAndGet(); }
            catch(LabProtocol.Fault expected) { refused.incrementAndGet(); } };
        Thread one=new Thread(consume), two=new Thread(consume); one.start(); two.start(); one.join(2000); two.join(2000);
        check(!one.isAlive() && !two.isAlive()); check(won.get()==1 && refused.get()==1);
        fault(LabProtocol.Reason.REF_CONSUMED, () -> operation.markEngineEntered());
        check(!finish(operation,true).engineEntered);
    }
    static void replies() throws Exception {
        Clock clock=new Clock(); IssuedEditor operation=operation(clock,8,15100);
        IssuedEditor.Lease lease=operation.issue(binding("seed"),100);
        byte[] frame=LabReplies.reference(lease);check(frame.length==50);
        LabReplies.Reference reply=LabReplies.parseReference(frame);
        check(reply.expires==4100);check(Arrays.equals(reply.epoch(),lease.epoch()));
        check(Arrays.equals(reply.reference(),lease.reference()));
        reply.epoch()[0]=99;reply.reference()[0]=99;
        check(Arrays.equals(reply.reference(),lease.reference()));
        check(!reply.toString().contains("seed"));
        byte[] closed=LabReplies.channelClosed();check(closed.length==11&&closed[9]==(byte)0x82&&closed[10]==1);
        for(int size=0;size<frame.length;size++){
            final byte[] partial=Arrays.copyOf(frame,size);
            fault(LabProtocol.Reason.INVALID_FRAME,()->LabReplies.parseReference(partial));
        }
        for(int offset:new int[]{3,4,8,9}){
            byte[] bad=frame.clone();bad[offset]=(byte)255;
            fault(LabProtocol.Reason.INVALID_FRAME,()->LabReplies.parseReference(bad));
        }
        byte[] zeroExpiry=frame.clone();Arrays.fill(zeroExpiry,42,50,(byte)0);
        fault(LabProtocol.Reason.INVALID_FRAME,()->LabReplies.parseReference(zeroExpiry));
        fault(LabProtocol.Reason.INVALID_FRAME,()->LabReplies.parseReference(Arrays.copyOf(frame,51)));
    }
    public static void main(String[] args) throws Exception {
        codec(); references(); effects(); admissionAndRace(); replies();
        System.out.println("PASS lab pure transport core checks: "+checks);
    }
}
