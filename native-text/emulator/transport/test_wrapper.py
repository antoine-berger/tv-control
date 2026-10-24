#!/usr/bin/env python3
"""Compile actual common wrapper and owner against Java-only Android/engine fakes."""
import argparse
import os
from pathlib import Path
import re
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from owned_runtime import command

STUBS = {
    "android/os/SystemClock.java": "package android.os; public class SystemClock { public static long time=100; public static long uptimeMillis(){return time;} }",
    "android/os/Bundle.java": r'''
package android.os;
public class Bundle {
    public final java.util.Map<String,String> values=new java.util.HashMap<String,String>();
    public void putString(String key,String value) { values.put(key,value); }
    public String getString(String key) { return values.get(key); }
}
''',
    "android/content/Context.java": r'''
package android.content;
public class Context { public java.io.File getCacheDir() { return new java.io.File("/synthetic-fixture-cache"); } }
''',
    "android/accessibilityservice/AccessibilityServiceInfo.java": r'''
package android.accessibilityservice;
public class AccessibilityServiceInfo { public static final int FLAG_REPORT_VIEW_IDS=16; public int flags; }
''',
    "android/view/accessibility/AccessibilityNodeInfo.java": r'''
package android.view.accessibility;
public class AccessibilityNodeInfo {
    public static final int FOCUS_INPUT=1;
    public int recycled;
    public AccessibilityNodeInfo findFocus(int kind) {
        if(kind!=FOCUS_INPUT) throw new IllegalStateException("wrong focus kind");
        return new AccessibilityNodeInfo();
    }
    public void recycle() { recycled++; }
}
''',
    "android/app/UiAutomation.java": r'''
package android.app;
public class UiAutomation {
    public static final int FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES=1;
    public android.accessibilityservice.AccessibilityServiceInfo info=new android.accessibilityservice.AccessibilityServiceInfo();
    public android.accessibilityservice.AccessibilityServiceInfo getServiceInfo() { return info; }
    public void setServiceInfo(android.accessibilityservice.AccessibilityServiceInfo value) { info=value; }
    public android.view.accessibility.AccessibilityNodeInfo getRootInActiveWindow() { return new android.view.accessibility.AccessibilityNodeInfo(); }
}
''',
    "android/app/Instrumentation.java": r'''
package android.app;
public class Instrumentation {
    public static final java.util.List<String> events=new java.util.ArrayList<String>();
    public static boolean contextThrows,connectThrows,nullAutomation,finishThrows;
    public static int lastFlags;
    public int finishes, starts; public android.os.Bundle result;
    public void onCreate(android.os.Bundle args) {}
    public void onStart() {}
    public void start() { starts++; }
    public android.content.Context getTargetContext() {
        if(contextThrows) throw new IllegalStateException("fixture-private-value");
        return new android.content.Context();
    }
    public UiAutomation getUiAutomation(int flags) {
        events.add("connect"); lastFlags=flags;
        android.os.SystemClock.time+=1000;
        if(connectThrows) throw new IllegalStateException("fixture-private-value");
        return nullAutomation ? null : new UiAutomation();
    }
    public void finish(int code,android.os.Bundle value) {
        events.add("finish"); finishes++; result=value;
        if(code!=0 || finishThrows) throw new IllegalStateException("finish fixture");
    }
}
''',
    "org/tvcontrol/nativeinput/TextEngine.java": r'''
package org.tvcontrol.nativeinput;
public final class TextEngine {
    public interface FocusSource { android.view.accessibility.AccessibilityNodeInfo focused(); }
    public interface ResultSink { void status(String value); }
    static int calls,mode; static String status="verified",sawToken;
    public static void runOnce(java.io.File base,String token,FocusSource source,ResultSink output) {
        android.app.Instrumentation.events.add("engine"); calls++; sawToken=token;
        if(token==null) { output.status("refused"); return; }
        if(mode==1) throw new IllegalStateException("fixture-private-value");
        if(mode==4) throw new AssertionError("fixture-private-value");
        if(mode==2) return;
        android.view.accessibility.AccessibilityNodeInfo node=source.focused();
        if(node==null) throw new IllegalStateException("fixture focus"); node.recycle();
        output.status(status);
        if(mode==3) output.status(status);
    }
}
''',
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--java", required=True, type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent
    modern = root.parent.parent / "modern/src/org/tvcontrol/nativeinput"
    owner = (modern / "NativePendingRequest.java").read_text()
    codec = (root / "src/org/tvcontrol/nativeinput/LabProtocol.java").read_text()
    def reasons(source):
        match = re.search(r"enum Reason \{([^}]+)\}", source)
        return {part.strip() for part in match.group(1).split(",")}
    if reasons(owner) != reasons(codec):
        raise RuntimeError("wrapper/core reason schema mismatch")
    env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C"}
    with tempfile.TemporaryDirectory(prefix="lab-wrapper-host-") as directory:
        work = Path(directory)
        stubs = []
        for name, value in STUBS.items():
            source = work / name
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text(value)
            stubs.append(source)
        sources = stubs + [modern / name for name in ("NativeTextInstrumentation.java", "NativePendingRequest.java")]
        sources += [root / "src/org/tvcontrol/nativeinput" / name
                    for name in ("LabProtocol.java", "EditorBinding.java", "IssuedEditor.java")]
        sources += [root / "tests/WrapperLifecycleTest.java"]
        command([args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main", "-encoding", "UTF-8",
                 "-source", "8", "-target", "8", "-Xlint:-options", "-d", work, *sources], env, 30)
        result = command([args.java, "-cp", work, "org.tvcontrol.nativeinput.WrapperLifecycleTest"], env, 10)
        if not re.fullmatch(r"PASS actual common wrapper host checks: [0-9]+\n", result):
            raise RuntimeError("invalid common-wrapper host result")
        print(result.strip())


if __name__ == "__main__":
    main()
