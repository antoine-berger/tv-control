#!/usr/bin/env python3
"""Actual lab issuer against explicit Android/unique-focus fakes; no Android runtime."""
import argparse
import os
from pathlib import Path
import re
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from owned_runtime import command
from test_wrapper import STUBS


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--java", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent
    stubs = dict(STUBS)
    stubs["android/graphics/Rect.java"] = "package android.graphics; public class Rect { public int left,top,right,bottom; }"
    stubs["android/view/accessibility/AccessibilityNodeInfo.java"] = r'''
package android.view.accessibility;
public class AccessibilityNodeInfo {
    public static final int FOCUS_INPUT=1,ACTION_SET_TEXT=2097152;
    public static AccessibilityNodeInfo focused;
    public String text="seed",hint,pkg="fixture",resource="fixture:id/editor",role="android.widget.EditText";
    public boolean focus=true,editable=true,enabled=true,visible=true,password,showingHint,unique=true,refreshOk=true;
    public int type=1,actions=ACTION_SET_TEXT,window=7,left=1,top=2,right=100,bottom=40,start=4,end=4,textReads,recycled,mutations;
    public Runnable onRefresh;
    public AccessibilityNodeInfo findFocus(int kind) { return focused; }
    public void recycle() { recycled++; }
    public boolean isFocused(){return focus;} public boolean isEditable(){return editable;}
    public boolean isEnabled(){return enabled;} public boolean isVisibleToUser(){return visible;}
    public boolean isPassword(){return password;} public int getInputType(){return type;}
    public boolean isShowingHintText(){return showingHint;} public CharSequence getHintText(){return hint;}
    public CharSequence getPackageName(){return pkg;} public String getViewIdResourceName(){return resource;}
    public CharSequence getClassName(){return role;} public int getActions(){return actions;}
    public int getWindowId(){return window;} public int getTextSelectionStart(){return start;} public int getTextSelectionEnd(){return end;}
    public boolean refresh(){if(onRefresh!=null)onRefresh.run();return refreshOk;}
    public CharSequence getText(){textReads++;return text;}
    public void getBoundsInScreen(android.graphics.Rect value){value.left=left;value.top=top;value.right=right;value.bottom=bottom;}
}
'''
    stubs["org/tvcontrol/nativeinput/TextEngine.java"] = r'''
package org.tvcontrol.nativeinput;
public final class TextEngine {
    public interface FocusSource { android.view.accessibility.AccessibilityNodeInfo focused(); }
    public interface ResultSink { void status(String value); }
    public static void runOnce(java.io.File base,String token,FocusSource source,ResultSink sink){throw new AssertionError("issuer must not enter engine");}
    static boolean sensitiveInputType(int type) {
        int kind=type&15,variation=type&4080;
        return kind==1&&(variation==128||variation==144||variation==224)||kind==2&&variation==16;
    }
    static boolean uniqueFocus(android.view.accessibility.AccessibilityNodeInfo node){return node.unique;}
}
'''
    modern = root.parent.parent / "modern/src/org/tvcontrol/nativeinput"
    core = root / "src/org/tvcontrol/nativeinput"
    with tempfile.TemporaryDirectory(prefix="lab-issuer-host-") as directory:
        work = Path(directory)
        sources = []
        for name, value in stubs.items():
            path = work / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(value)
            sources.append(path)
        sources += [modern / name for name in ("NativePendingRequest.java", "NativeTextInstrumentation.java")]
        sources += [core / name for name in ("LabProtocol.java", "EditorBinding.java", "IssuedEditor.java", "LabEditorIssuer.java")]
        sources += [root / "tests/EditorIssuerTest.java"]
        env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C"}
        command([args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main", "-source", "8", "-target", "8", "-Xlint:-options",
                 "-d", work, *sources], env, 30)
        result = command([args.java, "-cp", work, "org.tvcontrol.nativeinput.EditorIssuerTest"], env, 10)
        if not re.fullmatch(r"PASS lab editor issuer host checks: [0-9]+\n", result):
            raise RuntimeError("invalid issuer host result")
        print(result.strip())


if __name__ == "__main__":
    main()
