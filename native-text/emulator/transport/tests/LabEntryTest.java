package org.tvcontrol.nativeinput;

import android.app.Instrumentation;
import android.os.Bundle;
import java.lang.reflect.Modifier;

public final class LabEntryTest {
    static int checks;
    static void check(boolean value){checks++;if(!value)throw new AssertionError("lab entry contract");}
    static Bundle args(){Bundle value=new Bundle();value.putString("lab_instance","11111111111111111111111111111111");return value;}
    static void reset() {
        android.os.SystemClock.time=100;Instrumentation.events.clear();Instrumentation.contextThrows=false;
        Instrumentation.connectThrows=false;Instrumentation.nullAutomation=false;Instrumentation.finishThrows=false;
        LabSocketServer.opens=0;LabSocketServer.accepts=0;LabSocketServer.closes=0;LabSocketServer.failOpen=false;LabSocketServer.failClose=false;
        LabSocketChannel.admissions=0;LabSocketChannel.mode=0;LabPrivateRequest.opens=0;LabPrivateRequest.closes=0;LabPrivateRequest.cancels=0;
        LabEditorIssuer.reads=0;TextEngine.calls=0;TextEngine.mode=0;TextEngine.status="verified";
    }
    static LabSocketInstrumentation run(Bundle args) {
        LabSocketInstrumentation entry=new LabSocketInstrumentation();entry.onCreate(args);entry.onStart();
        check(entry.starts==1);check(entry.finishes==1);check(entry.result!=null);return entry;
    }
    static void result(LabSocketInstrumentation entry,String status,String reason,String entered,String cleanup) {
        check(status.equals(entry.result.getString("tv_native_text_v1")));
        check(reason.equals(entry.result.getString("tv_native_reason_v1")));
        check(entered.equals(entry.result.getString("tv_native_entered_v1")));
        check(cleanup.equals(entry.result.getString("tv_native_cleanup_v1")));
    }
    public static void main(String[] ignored) throws Exception {
        check(Modifier.isFinal(LabSocketInstrumentation.class.getModifiers()));
        check(LabSocketInstrumentation.class.getMethod("onStart").getDeclaringClass()==NativeTextInstrumentation.class);
        check(LabSocketInstrumentation.class.getMethod("finish",int.class,Bundle.class).getDeclaringClass()==Instrumentation.class);
        for(java.lang.reflect.Method method:LabSocketInstrumentation.class.getDeclaredMethods())
            check(!method.getName().equals("onStart")&&!method.getName().equals("finish"));
        reset();LabSocketInstrumentation cancel=run(args());result(cancel,"refused","CANCELLED","NO","VERIFIED");
        check(cancel.result.keySet().size()==6);check(cancel.statuses==1);check(cancel.ready.keySet().size()==3);
        check("READY".equals(cancel.ready.getString("tv_native_lab_ready_v1")));
        check(cancel.result.getString("tv_native_lab_instance_v1").equals(cancel.ready.getString("tv_native_lab_instance_v1")));
        check(LabSocketServer.opens==1&&LabSocketServer.accepts==1&&LabSocketServer.closes==1);
        check(LabSocketChannel.admissions==1);check(LabEditorIssuer.reads==0);check(LabPrivateRequest.opens==0);check(TextEngine.calls==0);
        reset();LabSocketChannel.mode=1;LabSocketInstrumentation good=run(args());result(good,"verified","NONE","YES","VERIFIED");
        check(TextEngine.calls==1);check(LabEditorIssuer.reads==1);check(LabPrivateRequest.opens==1);check(LabPrivateRequest.closes==1);
        check(good.result.keySet().size()==6);for(String text:good.result.values.values())check(!text.contains("private-token"));
        reset();LabSocketServer.failOpen=true;LabSocketInstrumentation denied=run(args());result(denied,"refused","POLICY_DENIED","NO","VERIFIED");
        check(denied.statuses==0);check(LabSocketServer.accepts==0);check(LabEditorIssuer.reads==0);check(TextEngine.calls==0);
        reset();LabSocketServer.failClose=true;LabSocketInstrumentation cleanup=run(args());result(cleanup,"uncertain","CLEANUP_UNVERIFIED","NO","UNVERIFIED");
        for(int bad=0;bad<5;bad++) {
            reset();Bundle invalid=args();
            if(bad==0)invalid.putString("token","private-not-allowed");
            if(bad==1)invalid.putString("text","private-not-allowed");
            if(bad==2)invalid.putString("lab_instance","UPPERCASE");
            if(bad==3)invalid=new Bundle();
            if(bad==4)invalid.putString("lab_instance",null);
            LabSocketInstrumentation entry=run(invalid);check(entry.statuses==0);check(LabSocketServer.opens==0);
            check(LabEditorIssuer.reads==0);check(TextEngine.calls==0);check("NOT_APPLICABLE".equals(entry.result.getString("tv_native_cleanup_v1")));
            check(entry.result.getString("tv_native_lab_instance_v1")==null);
        }
        reset();Instrumentation.connectThrows=true;LabSocketInstrumentation connect=run(args());check(connect.statuses==0);check(LabSocketServer.opens==0);
        check("NOT_APPLICABLE".equals(connect.result.getString("tv_native_cleanup_v1")));
        System.out.println("PASS actual final lab entry host checks: "+checks);
    }
}
