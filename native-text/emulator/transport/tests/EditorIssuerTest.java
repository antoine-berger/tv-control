package org.tvcontrol.nativeinput;

import android.app.UiAutomation;
import android.view.accessibility.AccessibilityNodeInfo;

public final class EditorIssuerTest {
    static int checks;
    static void check(boolean value){checks++;if(!value)throw new AssertionError("issuer invariant");}
    static UiAutomation automation(){UiAutomation value=new UiAutomation();value.info.flags=16;return value;}
    static LabEditorIssuer.Capture capture(UiAutomation value) throws LabProtocol.Fault {
        return LabEditorIssuer.capture(value,()->100,"fixture","fixture:id/editor");
    }
    static void refuse(AccessibilityNodeInfo node,boolean noText) throws Exception {
        AccessibilityNodeInfo.focused=node;
        try {capture(automation());throw new AssertionError("expected issuer refusal");}
        catch(LabProtocol.Fault failure){check(failure.reason==LabProtocol.Reason.EDITOR_UNSUPPORTED);}
        if(node!=null){check(node.recycled==1);check(node.mutations==0);if(noText)check(node.textReads==0);}
    }
    public static void main(String[] args) throws Exception {
        AccessibilityNodeInfo node=new AccessibilityNodeInfo(); AccessibilityNodeInfo.focused=node;
        LabEditorIssuer.Capture result=capture(automation());
        check(result.observedAt==100&&result.binding.window==7&&result.binding.before().equals("seed"));
        check(result.binding.left==1&&result.binding.right==100&&result.binding.selectionStart==4);
        check(node.recycled==1&&node.textReads==1&&node.mutations==0);
        node.text="changed";node.right=9;check(result.binding.before().equals("seed")&&result.binding.right==100);
        check(!result.toString().contains("seed")&&!result.binding.toString().contains("seed"));
        for(int condition=0;condition<18;condition++){
            node=new AccessibilityNodeInfo();
            if(condition==0)node.focus=false;
            if(condition==1)node.editable=false;
            if(condition==2)node.enabled=false;
            if(condition==3)node.visible=false;
            if(condition==4)node.password=true;
            if(condition==5)node.type=129;
            if(condition==6)node.type=145;
            if(condition==7)node.type=225;
            if(condition==8)node.type=18;
            if(condition==9)node.showingHint=true;
            if(condition==10)node.hint="fixture hint";
            if(condition==11)node.pkg="other";
            if(condition==12)node.resource="other:id/editor";
            if(condition==13)node.role="other.Editor";
            if(condition==14)node.actions=0;
            if(condition==15)node.unique=false;
            if(condition==16)node.refreshOk=false;
            if(condition==17)node.pkg=null;
            refuse(node,true);
        }
        for(int condition=0;condition<6;condition++){
            node=new AccessibilityNodeInfo();
            if(condition==0)node.text=null;
            if(condition==1)node.window=-1;
            if(condition==2)node.right=node.left;
            if(condition==3)node.start=1;
            if(condition==4)node.end=1;
            if(condition==5)node.text=new String(new char[257]);
            refuse(node,false);
        }
        node=new AccessibilityNodeInfo();final AccessibilityNodeInfo changing=node;
        node.onRefresh=()->changing.resource="other:id/editor";refuse(node,true);
        refuse(null,true);
        node=new AccessibilityNodeInfo();AccessibilityNodeInfo.focused=node;
        UiAutomation missingIds=automation();missingIds.info.flags=0;
        try{capture(missingIds);throw new AssertionError("missing ids must refuse");}
        catch(LabProtocol.Fault failure){check(node.textReads==0&&node.recycled==0);}
        System.out.println("PASS lab editor issuer host checks: "+checks);
    }
}
