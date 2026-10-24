package org.tvcontrol.nativeinput;

import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

public final class SocketSessionTest {
    static int checks;
    static void check(boolean condition){checks++;if(!condition)throw new AssertionError("session contract");}
    static byte[] frame(int opcode) throws Exception {
        ByteArrayOutputStream bytes=new ByteArrayOutputStream();DataOutputStream out=new DataOutputStream(bytes);
        out.writeInt(6);out.writeInt(LabProtocol.MAGIC);out.writeByte(1);out.writeByte(opcode);return bytes.toByteArray();
    }
    static byte[] commit(byte[] ref, boolean wrong) throws Exception {
        LabReplies.Reference value=LabReplies.parseReference(ref);
        ByteArrayOutputStream bytes=new ByteArrayOutputStream();DataOutputStream out=new DataOutputStream(bytes);
        out.writeInt(43);out.writeInt(LabProtocol.MAGIC);out.writeByte(1);out.writeByte(2);
        byte[] epoch=value.epoch();if(wrong)epoch[0]^=1;
        out.write(epoch);out.write(value.reference());out.writeInt(1);out.writeByte('x');return bytes.toByteArray();
    }
    static final class Clock implements IssuedEditor.Clock {
        volatile long now=100;public long nowMillis(){return now;}
    }
    static final class Cache implements LabSocketSession.Store {
        int writes,closes;volatile int cancels;boolean failOpen,failClose,proof=true;
        final CountDownLatch marker=new CountDownLatch(1);
        public String open(IssuedEditor.PendingText request) throws Exception {
            writes++;check(request.expires==4100);check("seedx".equals(request.after()));
            if(failOpen)throw new LabProtocol.Fault(LabProtocol.Reason.IDENTITY_CHANGED);return "private-cache-token";
        }
        public void cancel(){cancels++;marker.countDown();}
        public void close() throws Exception {closes++;if(failClose)throw new Exception("not printed");}
        public boolean cleanupVerified(){return proof && closes==1;}
    }
    static final class Port implements LabSocketSession.Port {
        final Clock clock;int opens,ready,accepts,reads,writes,closes;
        int first=1;boolean wrongRef,malformed,expire,openFault,closeFault,proof=true;
        volatile int late;byte[] reply;final CountDownLatch waiting=new CountDownLatch(1), delivered=new CountDownLatch(1);
        LabSocketSession.Read completedOnStop;
        Port(Clock time){clock=time;}
        public void open(long deadline) throws Exception {opens++;check(deadline==15100);if(openFault)throw new LabProtocol.Fault(LabProtocol.Reason.POLICY_DENIED);}
        public void ready(){ready++;check(opens==1);}
        public void accept(long until){accepts++;check(ready==1);check(until==5100);}
        public LabSocketSession.Read read(long until,LabSocketChannel.StopSignal stop) throws Exception {
            reads++;
            if(reads==1){if(malformed)return LabSocketSession.Read.data(new byte[]{1});return LabSocketSession.Read.data(frame(first));}
            if(reads==2){if(expire)clock.now=4100;return LabSocketSession.Read.data(commit(reply,wrongRef));}
            waiting.countDown();
            if(completedOnStop!=null) {
                // Complete bytes/event were observed before cleanup. Schedule
                // the method return only after the real session requests stop.
                while(!stop.stopped())Thread.yield();
                return completedOnStop;
            }
            while(!stop.stopped() && late==0)Thread.sleep(1);
            if(stop.stopped())return LabSocketSession.Read.stopped();
            delivered.countDown();
            if(late==1)return LabSocketSession.Read.data(frame(3));
            if(late==2)return LabSocketSession.Read.data(frame(1));
            if(late==3)return LabSocketSession.Read.data(new byte[]{1});
            if(late==4)return LabSocketSession.Read.eof();
            if(late==6)return LabSocketSession.Read.stopped();
            throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);
        }
        public void write(byte[] frame,long until){writes++;reply=frame.clone();check(until==2100);}
        public void close() throws Exception {closes++;if(closeFault)throw new Exception("not printed");}
        public boolean cleanupVerified(){return proof && closes==1;}
    }
    static final class Fixture {
        final Clock clock=new Clock();final Cache cache=new Cache();final Port port=new Port(clock);
        final NativePendingRequest owner=new NativePendingRequest();final LabSocketSession session;int captures;
        Fixture() throws Exception {
            owner.startedUptime(100);owner.cache(new java.io.File("/unused"));owner.publicInstance("11111111111111111111111111111111");
            session=new LabSocketSession(owner,clock,new IssuedEditor.Entropy(){public void fill(byte[] value){java.util.Arrays.fill(value,(byte)1);}},port,cache,
                new LabSocketSession.Capture(){public LabSocketSession.Observed capture() throws Exception {
                    captures++;return new LabSocketSession.Observed(new EditorBinding("fixture","fixture:id/editor","android.widget.EditText",1,1,2,30,40,"seed",4,4),clock.now);
                }});
            owner.attach(session);
        }
        void prepare() throws Exception {check(session.prepare());check(port.waiting.await(1,TimeUnit.SECONDS));}
        NativePendingRequest.Terminal end(){return owner.finish();}
    }
    static void terminal(NativePendingRequest.Terminal value,String status,String reason,boolean entered,boolean clean){
        check(status.equals(value.status));check(reason.equals(value.reason.name()));check(value.entered==entered);
        check(value.cleanup.name().equals(clean?"VERIFIED":"UNVERIFIED"));
    }
    public static void main(String[] args) throws Exception {
        Fixture good=new Fixture();good.prepare();check(good.owner.enter());good.owner.result("verified");
        terminal(good.end(),"verified","NONE",true,true);check(good.captures==1);check(good.cache.writes==1);check(good.cache.cancels==0);
        Fixture refused=new Fixture();refused.prepare();check(refused.owner.enter());refused.owner.result("refused");
        terminal(refused.end(),"refused","ENGINE_REFUSED",true,true);
        Fixture cancel=new Fixture();cancel.port.first=3;check(!cancel.session.prepare());
        terminal(cancel.end(),"refused","CANCELLED",false,true);check(cancel.captures==0);check(cancel.cache.writes==0);check(cancel.port.writes==0);
        Fixture precommit=new Fixture();precommit.port.first=2;check(!precommit.session.prepare());terminal(precommit.end(),"refused","INVALID_FRAME",false,true);
        Fixture malformed=new Fixture();malformed.port.malformed=true;check(!malformed.session.prepare());terminal(malformed.end(),"refused","INVALID_FRAME",false,true);
        Fixture wrong=new Fixture();wrong.port.wrongRef=true;check(!wrong.session.prepare());terminal(wrong.end(),"refused","REF_UNKNOWN",false,true);check(wrong.cache.writes==0);
        Fixture expired=new Fixture();expired.port.expire=true;check(!expired.session.prepare());terminal(expired.end(),"refused","REF_EXPIRED",false,true);check(expired.cache.writes==0);
        Fixture policy=new Fixture();policy.port.openFault=true;check(!policy.session.prepare());terminal(policy.end(),"refused","POLICY_DENIED",false,true);check(policy.port.ready==0);check(policy.captures==0);
        Fixture cacheFailure=new Fixture();cacheFailure.cache.failOpen=true;check(!cacheFailure.session.prepare());terminal(cacheFailure.end(),"refused","IDENTITY_CHANGED",false,true);
        for(int event=1;event<=6;event++) {
            Fixture late=new Fixture();late.prepare();check(late.owner.enter());late.port.late=event;
            check(late.port.delivered.await(1,TimeUnit.SECONDS));
            check(late.cache.marker.await(1,TimeUnit.SECONDS));
            // Result may race with abort; both orders must retain the external event.
            late.owner.result("refused");
            NativePendingRequest.Terminal outcome=late.end();
            check("uncertain".equals(outcome.status));check(outcome.entered);check(outcome.cleanup==NativePendingRequest.Cleanup.VERIFIED);
            check(late.cache.cancels==1);
        }
        LabSocketSession.Read[] complete={LabSocketSession.Read.data(new byte[]{1}),
            LabSocketSession.Read.data(frame(1)),LabSocketSession.Read.data(frame(3)),
            LabSocketSession.Read.eof(),LabSocketSession.Read.stopped()};
        String[] reasons={"INVALID_FRAME","INVALID_FRAME","CANCELLED","TRANSPORT_LOST","NONE"};
        for(int race=0;race<complete.length;race++) {
            Fixture cleanupRace=new Fixture();cleanupRace.port.completedOnStop=complete[race];
            cleanupRace.prepare();check(cleanupRace.owner.enter());cleanupRace.owner.result("verified");
            terminal(cleanupRace.end(),race==4?"verified":"uncertain",reasons[race],true,true);
            check(cleanupRace.cache.cancels==0); // Never write a marker after stop.
            check(cleanupRace.cache.closes==1);check(cleanupRace.port.closes==1);
        }
        Fixture noWrite=new Fixture();noWrite.prepare();noWrite.port.late=1;
        check(noWrite.port.delivered.await(1,TimeUnit.SECONDS));
        // Wait for cancellation to reach the synchronized core via the private marker.
        long bounded=System.nanoTime()+TimeUnit.SECONDS.toNanos(1);
        while(noWrite.cache.cancels==0 && System.nanoTime()<bounded)Thread.yield();
        check(noWrite.cache.cancels==1);check(!noWrite.owner.enter());terminal(noWrite.end(),"refused","CANCELLED",false,true);
        Fixture stale=new Fixture();stale.prepare();stale.clock.now=4100;
        check(!stale.owner.enter());terminal(stale.end(),"refused","REF_EXPIRED",false,true);
        Fixture missingResult=new Fixture();missingResult.prepare();check(missingResult.owner.enter());
        terminal(missingResult.end(),"uncertain","TRANSPORT_LOST",true,true);
        for(int failure=1;failure<=4;failure++) {
            Fixture bad=new Fixture();bad.prepare();check(bad.owner.enter());bad.owner.result("verified");
            bad.port.closeFault=failure==1;bad.port.proof=failure!=2;bad.cache.failClose=failure==3;bad.cache.proof=failure!=4;
            terminal(bad.end(),"uncertain","CLEANUP_UNVERIFIED",true,false);check(bad.port.closes==1);check(bad.cache.closes==1);
        }
        System.out.println("PASS actual lab session host checks: "+checks);
    }
}
