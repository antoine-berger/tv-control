package org.tvcontrol.nativeinput;

import android.app.Instrumentation;
import android.app.UiAutomation;
import android.os.Bundle;

/** Exercises inherited real onStart/finish and real owner; engine/Android are fakes. */
public final class WrapperLifecycleTest {
    static int checks;
    static final String INSTANCE="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    static final String TOKEN="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    static void check(boolean value) { checks++; if(!value) throw new AssertionError("common wrapper invariant"); }
    static void reset() {
        Instrumentation.events.clear(); Instrumentation.contextThrows=false; Instrumentation.connectThrows=false;
        Instrumentation.nullAutomation=false; Instrumentation.finishThrows=false;
        TextEngine.calls=0; TextEngine.mode=0; TextEngine.status="verified"; TextEngine.sawToken=null;
    }
    static class Life implements NativePendingRequest.Lifecycle {
        int closes; boolean closed,gate=true,cleanup=true,closeThrows,proofThrows,resultThrows,terminalThrows,failedThrows,badStatus,badReason,lateCancelReason;
        String status="refused",reason="REF_UNKNOWN";
        public boolean beforeEntry() { Instrumentation.events.add("gate"); return gate; }
        public void result(String value) {
            Instrumentation.events.add("result");
            if(resultThrows) throw new IllegalStateException("fixture-private-value");
            status=value; reason="verified".equals(value) ? "NONE" : "refused".equals(value) ? "ENGINE_REFUSED" : "ENGINE_UNCERTAIN";
        }
        public void failed() {
            status="uncertain"; reason="TRANSPORT_LOST";
            if(failedThrows) throw new IllegalStateException("fixture-private-value");
        }
        public void close() {
            Instrumentation.events.add("close"); closes++; closed=true;
            if(closeThrows) throw new AssertionError("fixture-private-value");
        }
        public boolean cleanupVerified() {
            Instrumentation.events.add("proof"); check(closed);
            if(proofThrows) throw new IllegalStateException("fixture-private-value");
            return cleanup;
        }
        public String terminalStatus() {
            Instrumentation.events.add("terminal"); check(closed);
            if(terminalThrows) throw new IllegalStateException("fixture-private-value");
            return badStatus ? "fixture-private-value" : status;
        }
        public String terminalReason() { return badReason ? "fixture-private-value" : lateCancelReason ? "CANCELLED" : reason; }
    }
    static final class Harness extends NativeTextInstrumentation {
        final Life life; boolean throwIngress,allow=true,dropToken;
        Harness(Life value) { life=value; }
        // Deliberately does NOT override onStart/finish: actual common lifecycle runs.
        @Override protected boolean prepareRequest(UiAutomation automation,NativePendingRequest pending) {
            Instrumentation.events.add("ingress");
            check((automation.getServiceInfo().flags & 16)!=0);
            pending.publicInstance(INSTANCE); pending.attach(life);
            if(throwIngress) throw new AssertionError("fixture-private-value");
            if(!dropToken) pending.token(TOKEN);
            return allow;
        }
    }
    static void launch(NativeTextInstrumentation wrapper) {
        Bundle input=new Bundle(); input.putString("token","fixture-private-argv-value");
        wrapper.onCreate(input); check(wrapper.starts==1); wrapper.onStart(); check(wrapper.finishes==1);
        check(!wrapper.result.values.toString().contains("fixture-private"));
    }
    static void outcome(Harness wrapper,String status,String cleanup,String entered) {
        check(status.equals(wrapper.result.getString("tv_native_text_v1")));
        check(cleanup.equals(wrapper.result.getString("tv_native_cleanup_v1")));
        check(entered.equals(wrapper.result.getString("tv_native_entered_v1")));
        check("1".equals(wrapper.result.getString("tv_native_lab_protocol_v1")));
        check(INSTANCE.equals(wrapper.result.getString("tv_native_lab_instance_v1")));
        check(wrapper.life.closes==1);
        check(Instrumentation.events.indexOf("close")<Instrumentation.events.indexOf("proof"));
        check(Instrumentation.events.indexOf("proof")<Instrumentation.events.indexOf("terminal"));
        check(Instrumentation.events.indexOf("terminal")<Instrumentation.events.indexOf("finish"));
    }
    static void lifecycle() throws Exception {
        reset(); Life life=new Life(); Harness success=new Harness(life); launch(success);
        outcome(success,"verified","VERIFIED","YES"); check(TextEngine.calls==1);
        check(TOKEN.equals(TextEngine.sawToken)); check(Instrumentation.lastFlags==UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES);
        check("NONE".equals(success.result.getString("tv_native_reason_v1")));
        reset(); Harness refused=new Harness(new Life()); refused.allow=false; launch(refused);
        outcome(refused,"refused","VERIFIED","NO"); check(TextEngine.calls==0);
        reset(); Harness cancelled=new Harness(new Life()); cancelled.life.gate=false; launch(cancelled);
        outcome(cancelled,"refused","VERIFIED","NO"); check(TextEngine.calls==0);
        for(int failure=0;failure<14;failure++) {
            reset(); Harness wrapper=new Harness(new Life());
            switch(failure) {
                case 0: wrapper.throwIngress=true; break;
                case 1: TextEngine.mode=1; break;
                case 2: TextEngine.mode=2; break;
                case 3: TextEngine.mode=3; break;
                case 4: wrapper.life.closeThrows=true; break;
                case 5: wrapper.life.cleanup=false; break;
                case 6: wrapper.life.proofThrows=true; break;
                case 7: wrapper.life.terminalThrows=true; break;
                case 8: wrapper.life.resultThrows=true; wrapper.life.failedThrows=true; break;
                case 9: TextEngine.mode=4; break;
                case 10: wrapper.dropToken=true; break;
                case 11: wrapper.life.badStatus=true; break;
                case 12: wrapper.life.badReason=true; break;
                case 13: wrapper.life.lateCancelReason=true; break;
            }
            launch(wrapper);
            outcome(wrapper,"uncertain",failure>=4 && failure<=6 ? "UNVERIFIED" : "VERIFIED",
                failure==0 || failure==10 ? "NO" : "YES");
            check(TextEngine.calls<=1);
        }
        for(String bad:new String[]{"fixture-private-value",null}) {
            reset(); TextEngine.status=bad; Harness wrapper=new Harness(new Life()); launch(wrapper);
            outcome(wrapper,"uncertain","VERIFIED","YES");
        }
        reset(); NativeTextInstrumentation legacy=new NativeTextInstrumentation();
        legacy.onCreate(new Bundle()); legacy.onStart();
        check("refused".equals(legacy.result.getString("tv_native_text_v1")));
        check("NOT_APPLICABLE".equals(legacy.result.getString("tv_native_cleanup_v1")));
        check(legacy.result.getString("tv_native_lab_instance_v1")==null);
        for(boolean context:new boolean[]{false,true}) {
            reset(); Instrumentation.contextThrows=context; Instrumentation.connectThrows=!context;
            Harness wrapper=new Harness(new Life()); launch(wrapper);
            check("uncertain".equals(wrapper.result.getString("tv_native_text_v1")));
            check(wrapper.life.closes==0 && TextEngine.calls==0);
        }
        reset(); Harness finishFailure=new Harness(new Life()); Instrumentation.finishThrows=true;
        try { launch(finishFailure); throw new AssertionError("finish must fail"); }
        catch(IllegalStateException expected) { check(finishFailure.finishes==1 && finishFailure.life.closes==1); }
        NativePendingRequest owner=new NativePendingRequest(); Life once=new Life();
        owner.cache(new java.io.File("/synthetic-fixture-cache")); owner.publicInstance(INSTANCE);
        owner.attach(once); owner.token(TOKEN); check(owner.enter()); owner.result("verified");
        NativePendingRequest.Terminal first=owner.finish(), second=owner.finish();
        check(first.status.equals("verified") && second.status.equals("uncertain"));
        check(second.cleanup==NativePendingRequest.Cleanup.UNVERIFIED && once.closes==1);
        check(!first.toString().contains(TOKEN));
    }
    static void boundary() throws Exception {
        for(boolean afterEntry:new boolean[]{false,true}) {
            reset(); final IssuedEditor.Clock clock=() -> 100;
            final IssuedEditor operation=new IssuedEditor(clock,bytes -> java.util.Arrays.fill(bytes,(byte)1),15100);
            EditorBinding binding=new EditorBinding("pkg","id","role",1,0,0,1,1,"seed",4,4);
            IssuedEditor.Lease ref=operation.issue(binding,100);
            java.io.ByteArrayOutputStream body=new java.io.ByteArrayOutputStream();
            java.io.DataOutputStream data=new java.io.DataOutputStream(body);
            data.writeInt(LabProtocol.MAGIC); data.writeByte(1); data.writeByte(2);
            data.write(ref.epoch()); data.write(ref.reference()); data.writeInt(1); data.writeByte('x');
            java.io.ByteArrayOutputStream bytes=new java.io.ByteArrayOutputStream();
            data=new java.io.DataOutputStream(bytes); data.writeInt(body.size()); data.write(body.toByteArray());
            operation.consume(LabProtocol.decode(bytes.toByteArray(),new LabProtocol.InputBudget()));
            final boolean late=afterEntry;
            Life life=new Life() {
                IssuedEditor.Outcome terminal;
                public boolean beforeEntry() {
                    if(!late) operation.abort(LabProtocol.Reason.CANCELLED);
                    try { operation.markEngineEntered(); }
                    catch(LabProtocol.Fault refused) { return false; }
                    operation.abort(LabProtocol.Reason.CANCELLED); return true;
                }
                public void result(String value) {
                    try { operation.engineResult(value); } catch(LabProtocol.Fault error) { throw new IllegalStateException("fixed callback"); }
                }
                public void close() {
                    super.close();
                    try { operation.beginCleanup(); terminal=operation.finishCleanup(true); }
                    catch(LabProtocol.Fault error) { throw new IllegalStateException("fixed cleanup"); }
                }
                public String terminalStatus() { super.terminalStatus(); return terminal.status.name().toLowerCase(java.util.Locale.ROOT); }
                public String terminalReason() { return terminal.reason.name(); }
            };
            Harness wrapper=new Harness(life); launch(wrapper);
            outcome(wrapper,afterEntry ? "uncertain" : "refused","VERIFIED",afterEntry ? "YES" : "NO");
            check(TextEngine.calls==(afterEntry ? 1 : 0));
        }
    }
    public static void main(String[] args) throws Exception {
        lifecycle(); boundary();
        System.out.println("PASS actual common wrapper host checks: "+checks);
    }
}
