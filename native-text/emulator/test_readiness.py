#!/usr/bin/env python3
"""Execute actual Java poll/predicate against fake clock/nodes; not Android runtime."""
import argparse
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument("--java", required=True)
args = parser.parse_args()
root = Path(__file__).parent
source = (root / "src/org/tvcontrol/nativeinput/RuntimeProof.java").read_text()
predicate = source[source.index("    private boolean fixtureReady() {"):source.index("    public void testFixture()")]
predicate = predicate.replace("AccessibilityNodeInfo", "Node")
harness = r'''
package org.tvcontrol.nativeinput;
public final class ReadinessTest {
    static int checks;
    static void check(boolean v) { checks++; if (!v) throw new AssertionError(); }
    static final class FakeClock implements Readiness.Clock {
        long time; int sleeps;
        public long now() { return time; }
        public void sleep(long n) { check(n > 0 && n <= 200); time += n; sleeps++; }
    }
    enum RootCategory { FIXTURE, SYSTEM, MISSING }
    static final String ID = "org.tvcontrol.fixture:id/editor";
    RootCategory rootCategory = RootCategory.FIXTURE;
    Node selected = new Node();
    void observeDeviceState() {}
    Node editor(String id) { return selected; }
    static final class Rect { boolean empty; boolean isEmpty() { return empty; } }
    static final class Node {
        String pkg="org.tvcontrol.fixture", id=ID, cls="android.widget.EditText";
        boolean focused=true, editable=true, enabled=true, visible=true, empty;
        int recycled;
        void getBoundsInScreen(Rect r) { r.empty=empty; }
        CharSequence getPackageName() { return pkg; }
        String getViewIdResourceName() { return id; }
        CharSequence getClassName() { return cls; }
        boolean isFocused() { return focused; }
        boolean isEditable() { return editable; }
        boolean isEnabled() { return enabled; }
        boolean isVisibleToUser() { return visible; }
        void recycle() { recycled++; }
    }
PREDICATE
    public static void main(String[] args) {
        FakeClock c = new FakeClock(); final int[] calls={0};
        check(Readiness.await(() -> ++calls[0] == 3, c));
        check(calls[0]==3 && c.time==400);
        c=new FakeClock(); calls[0]=0;
        check(!Readiness.await(() -> { calls[0]++; return false; }, c));
        check(c.time==20000 && calls[0]==100);
        final FakeClock slow=new FakeClock();
        check(!Readiness.await(() -> { slow.time=20000; return true; }, slow));
        final FakeClock backwards=new FakeClock();
        check(!Readiness.await(() -> { backwards.time=-1; return true; }, backwards));
        ReadinessTest t=new ReadinessTest();
        check(t.fixtureReady()); check(t.selected.recycled==1);
        t.rootCategory=RootCategory.SYSTEM; check(!t.fixtureReady()); check(t.selected.recycled==2);
        t.selected=null; check(!t.fixtureReady());
        for(int i=0;i<9;i++) {
            t=new ReadinessTest(); Node n=t.selected;
            switch(i) {
                case 0:n.pkg=null;break; case 1:n.id="wrong";break; case 2:n.cls=null;break;
                case 3:n.focused=false;break; case 4:n.editable=false;break; case 5:n.enabled=false;break;
                case 6:n.visible=false;break; case 7:n.empty=true;break; case 8:n.pkg="other";break;
            }
            check(!t.fixtureReady()); check(n.recycled==1);
        }
        System.out.println("Readiness poll/predicate checks passed: " + checks + " (algorithm-only)");
    }
}
'''.replace("PREDICATE", predicate)
with tempfile.TemporaryDirectory(prefix="native-readiness-test-") as tmp:
    tmp = Path(tmp)
    harness_path = tmp / "ReadinessTest.java"
    harness_path.write_text(harness)
    subprocess.run([args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main", "-d", str(tmp),
                    str(root / "src/org/tvcontrol/nativeinput/Readiness.java"), str(harness_path)], check=True, timeout=30)
    subprocess.run([args.java, "-cp", str(tmp), "org.tvcontrol.nativeinput.ReadinessTest"], check=True, timeout=10)
# No text/action/selection access in readiness; negative fixtures remain eligible.
for forbidden in ("getText(", "performAction(", "isPassword(", "getHintText(", "getTextSelection"):
    assert forbidden not in predicate
runner = (root / "owned_proof.py").read_text()
assert runner.index('"mode", "ready"') < runner.index('shell("input", "text", "Tanya")')
assert runner.count('shell("input", "text", "Tanya")') == 1
assert 'if readiness["proof"] != "passed"' in runner
