#!/usr/bin/env python3
"""Actual final lab entrypoint/common onStart/adapter/session under explicit Android/IPC fakes."""
import argparse
import os
from pathlib import Path
import re
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from owned_runtime import command
from test_wrapper import STUBS

FAKES = {
    "org/tvcontrol/nativeinput/LabSocketServer.java": r'''
package org.tvcontrol.nativeinput;
final class LabSocketServer {
    static int opens,accepts,closes;static boolean failOpen,failClose;boolean closed;
    LabSocketServer(String instance,IssuedEditor.Clock clock,long deadline){if(deadline!=15100)throw new AssertionError("deadline");}
    void open() throws LabProtocol.Fault {opens++;if(failOpen)throw new LabProtocol.Fault(LabProtocol.Reason.POLICY_DENIED);}
    LabSocketChannel accept(long until){accepts++;if(until!=6100)throw new AssertionError("accept bound");return new LabSocketChannel();}
    void close() throws Exception {closes++;closed=true;if(failClose)throw new Exception("private fake diagnostic");}
    boolean cleanupVerified(){return closed&&!failClose;}
}
''',
    "org/tvcontrol/nativeinput/LabSocketChannel.java": r'''
package org.tvcontrol.nativeinput;
final class LabSocketChannel {
    interface StopSignal {boolean stopped();} enum Kind {DATA,EOF,STOP}
    static int mode,admissions;static byte[] reference;
    int reads;
    static final class FrameRead {final Kind kind;final byte[] frame;FrameRead(Kind k,byte[] f){kind=k;frame=f;}byte[] bytes(){return frame.clone();}}
    void admitPeer(int uid){if(uid!=2000)throw new AssertionError("shell UID");admissions++;}
    FrameRead readFrame(long until,StopSignal stop) throws Exception {
        if(admissions==0)throw new AssertionError("unadmitted IO");reads++;
        if(reads==1)return new FrameRead(Kind.DATA,SocketSessionTest.frame(mode==0?3:1));
        if(reads==2)return new FrameRead(Kind.DATA,SocketSessionTest.commit(reference,false));
        while(!stop.stopped())Thread.sleep(1);
        return new FrameRead(Kind.STOP,null);
    }
    void writeFrame(byte[] frame,long until){reference=frame.clone();}
}
''',
    "org/tvcontrol/nativeinput/LabPrivateRequest.java": r'''
package org.tvcontrol.nativeinput;
final class LabPrivateRequest {
    static int opens,closes,cancels;boolean closed;
    LabPrivateRequest(java.io.File cache){}
    void open(IssuedEditor.PendingText request){opens++;if(request.expires!=5100)throw new AssertionError("immutable expiry");}
    String token(){return "private-token-not-argv";}
    void cancel(){cancels++;}
    void close(){closes++;closed=true;}
    boolean cleanupVerified(){return closed;}
}
''',
    "org/tvcontrol/nativeinput/LabEditorIssuer.java": r'''
package org.tvcontrol.nativeinput;
final class LabEditorIssuer {
    static int reads;
    static final class Capture {EditorBinding binding;long observedAt;}
    static Capture capture(android.app.UiAutomation automation,IssuedEditor.Clock clock,String pkg,String resource) throws Exception {
        reads++;if(!pkg.equals("org.tvcontrol.fixture")||!resource.equals("org.tvcontrol.fixture:id/editor"))throw new AssertionError("owned fixture only");
        Capture value=new Capture();value.observedAt=clock.nowMillis();
        value.binding=new EditorBinding(pkg,resource,"android.widget.EditText",1,1,2,30,40,"seed",4,4);return value;
    }
}
''',
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--java", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent
    core = root / "src/org/tvcontrol/nativeinput"
    modern = root.parent.parent / "modern/src/org/tvcontrol/nativeinput"
    stubs = dict(STUBS)
    stubs["android/os/Bundle.java"] = stubs["android/os/Bundle.java"].replace(
        "public String getString", "public java.util.Set<String> keySet(){return values.keySet();} public boolean containsKey(String key){return values.containsKey(key);} public String getString")
    stubs["android/app/Instrumentation.java"] = stubs["android/app/Instrumentation.java"].replace(
        "public int finishes", "public int statuses; public android.os.Bundle ready; public void sendStatus(int code,android.os.Bundle value){if(code!=1)throw new AssertionError(\"READY code\");statuses++;ready=value;} public int finishes")
    stubs.update(FAKES)
    env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C"}
    with tempfile.TemporaryDirectory(prefix="lab-entry-host-") as directory:
        work = Path(directory)
        sources = []
        for name, value in stubs.items():
            path = work / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(value)
            sources.append(path)
        sources += [modern / name for name in ("NativePendingRequest.java", "NativeTextInstrumentation.java")]
        sources += [core / name for name in ("LabProtocol.java", "EditorBinding.java", "IssuedEditor.java", "LabReplies.java",
                    "LabSocketSession.java", "LabAndroidIngress.java", "LabSocketInstrumentation.java")]
        sources += [root / "tests/SocketSessionTest.java", root / "tests/LabEntryTest.java"]
        command([args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main", "-source", "8", "-target", "8",
                 "-Xlint:-options", "-d", work, *sources], env, 30)
        result = command([args.java, "-cp", work, "org.tvcontrol.nativeinput.LabEntryTest"], env, 15)
        if not re.fullmatch(r"PASS actual final lab entry host checks: [0-9]+\n", result):
            raise RuntimeError("invalid lab entry host result")
        print(result.strip())


if __name__ == "__main__":
    main()
