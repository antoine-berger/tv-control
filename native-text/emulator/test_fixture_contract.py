#!/usr/bin/env python3
"""Actual fixture/proof/engine predicates with host Java fakes; not Android proof."""
import argparse
import os
from pathlib import Path
import tempfile
from owned_runtime import command


def method(source, signature):
    start = source.index(signature)
    opening = source.index("{", start)
    depth = 1
    end = opening + 1
    while depth:
        if source[end] == "{":
            depth += 1
        elif source[end] == "}":
            depth -= 1
        end += 1
    return source[start:end]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--java", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).parent
    fixture = (root / "fixture/src/org/tvcontrol/fixture/FixtureActivity.java").read_text()
    proof = (root / "src/org/tvcontrol/nativeinput/RuntimeProof.java").read_text()
    engine = (root.parent / "src/org/tvcontrol/nativeinput/TextEngine.java").read_text()
    configure = method(fixture, "    private static void configureEditors(").replace("EditText", "Editor")
    readback = method(proof, "    private static void assertEditorText(").replace("AccessibilityNodeInfo", "Node")
    other_readback = method(proof, "    private void assertOtherUnchanged(").replace("AccessibilityNodeInfo", "Node")
    matches = method(engine, "        boolean matches(").replace("AccessibilityNodeInfo", "Node")
    sensitive = method(engine, "    static boolean sensitiveInputType(")
    before = proof[proof.index('        boolean emptyNull = "empty-null".equals(mode);'):proof.index('        if ("key-remap".equals(mode))')]
    before += proof[proof.index('        if ("hint".equals(mode)) assertEquals'):proof.index('        String after = before +')]
    harness = r'''
public final class FixtureContractTest {
    static int checks;
    static void assertTrue(String reason, boolean value) { checks++; if (!value) throw new AssertionError("fixture contract"); }
    static void assertNotNull(String reason, Object value) { assertTrue(reason, value != null); }
    static void assertEquals(Object expected, Object actual) { assertTrue("equal", expected == null ? actual == null : expected.equals(actual)); }
    static void rejects(Runnable call) {
        try { call.run(); } catch (AssertionError expected) { checks++; return; }
        throw new AssertionError("expected rejection");
    }
    static class InputType {
        static final int TYPE_MASK_CLASS=15, TYPE_MASK_VARIATION=4080, TYPE_CLASS_TEXT=1,
            TYPE_CLASS_NUMBER=2, TYPE_TEXT_VARIATION_PASSWORD=128,
            TYPE_TEXT_VARIATION_VISIBLE_PASSWORD=144, TYPE_TEXT_VARIATION_WEB_PASSWORD=224,
            TYPE_NUMBER_VARIATION_PASSWORD=16;
    }
    static class Editor {
        String text="", hint; int selection, type=1; boolean focused;
        void setText(String value) { text=value; selection=0; }
        String getText() { return text; }
        void setSelection(int value) { selection=value; }
        void setHint(String value) { hint=value; }
        void setInputType(int value) { type=value; }
        void requestFocus() { focused=true; }
    }
    static class Rect {
        public boolean equals(Object value) { return value instanceof Rect; }
    }
    static class Node {
        String text, hint; int selection, type=1, refreshed, recycled; boolean password, refreshOk=true;
        Node(String value) { text=value; selection=value == null ? -1 : value.length(); }
        CharSequence getText() { return text; }
        CharSequence getHintText() { return hint; }
        boolean isFocused() { return true; }
        boolean isEditable() { return true; }
        boolean isEnabled() { return true; }
        boolean isVisibleToUser() { return true; }
        boolean isPassword() { return password; }
        boolean isShowingHintText() { return hint != null; }
        int getInputType() { return type; }
        int getTextSelectionStart() { return selection; }
        int getTextSelectionEnd() { return selection; }
        void getBoundsInScreen(Rect value) {}
        String getPackageName() { return "fixture"; }
        String getViewIdResourceName() { return "editor"; }
        String getClassName() { return "EditText"; }
        int getWindowId() { return 1; }
        boolean refresh() { refreshed++; return refreshOk; }
        void recycle() { recycled++; }
    }
    static class Request {
        String pkg="fixture", resource="editor", role="EditText"; int window=1; Rect bounds=new Rect();
        @MATCHES@
    }
    @SENSITIVE@
    @CONFIGURE@
    @READBACK@
    enum Stage { READBACK_CHECK, OTHER_LOOKUP }
    Stage stage=Stage.READBACK_CHECK;
    Node other;
    int otherLookups;
    Node editor(String id) {
        assertEquals("org.tvcontrol.fixture:id/other", id);
        otherLookups++; stage=Stage.OTHER_LOOKUP; return other;
    }
    @OTHER_READBACK@
    static String validateBefore(String mode, Node n) {
        @BEFORE@
        return before;
    }
    public static void main(String[] args) {
        String[] modes={"normal", "empty-null", "hint", "password", "visible-password", "selection", "focus", "stale-window", "cancel", "key-remap"};
        for (String mode:modes) {
            Editor editor=new Editor(), other=new Editor();
            configureEditors(mode, editor, other);
            boolean unseeded=mode.equals("empty-null") || mode.equals("hint") || mode.equals("key-remap");
            assertEquals(unseeded ? "" : "seed", editor.text);
            assertEquals(unseeded ? 0 : mode.equals("selection") ? 1 : 4, editor.selection);
            assertTrue("focus", editor.focused);
            assertEquals(mode.equals("hint") ? "fixture hint" : null, editor.hint);
            assertEquals(mode.equals("password") ? 129 : mode.equals("visible-password") ? 145 : 1, editor.type);
            assertEquals(mode.equals("focus") ? "other" : "", other.text);
            assertEquals(mode.equals("focus") ? 5 : 0, other.selection);
        }
        Node seed=new Node("seed");
        assertEquals("seed", validateBefore("normal", seed));
        rejects(() -> validateBefore("normal", new Node(null)));
        rejects(() -> validateBefore("normal", new Node("")));
        seed.selection=1;
        rejects(() -> validateBefore("normal", seed));
        assertEquals("seed", validateBefore("selection", seed));
        assertEquals("", validateBefore("empty-null", new Node(null)));
        rejects(() -> validateBefore("empty-null", new Node("")));
        assertEditorText(new Node(null), "", true);
        rejects(() -> assertEditorText(new Node(""), "", true));
        rejects(() -> assertEditorText(new Node(null), "", false));
        String after="seedTanya é🙂%s'";
        assertEditorText(new Node(after), after, false);
        rejects(() -> assertEditorText(new Node("seed"), after, false));
        Request request=new Request();
        assertTrue("seed matches", request.matches(new Node("seed"), "seed"));
        assertTrue("null refused", !request.matches(new Node(null), ""));
        Node hint=new Node("fixture hint"); hint.hint="fixture hint";
        assertTrue("hint refused", !request.matches(hint, "fixture hint"));
        Node password=new Node("seed"); password.type=129;
        assertTrue("password refused with text", !request.matches(password, "seed"));
        password.type=145;
        assertTrue("visible password refused with text", !request.matches(password, "seed"));
        FixtureContractTest proof=new FixtureContractTest();
        for (int invocation=1; invocation<=3; invocation++) {
            proof.other=new Node("other");
            proof.assertOtherUnchanged();
            assertEquals(invocation, proof.otherLookups);
            assertEquals(1, proof.other.refreshed); assertEquals(1, proof.other.recycled);
            assertEquals(Stage.READBACK_CHECK, proof.stage);
        }
        for (String changed:new String[]{"mutated", "", null}) {
            proof.other=new Node(changed);
            rejects(() -> proof.assertOtherUnchanged());
            assertEquals(1, proof.other.refreshed); assertEquals(1, proof.other.recycled);
        }
        proof.other=new Node("other"); proof.other.refreshOk=false;
        rejects(() -> proof.assertOtherUnchanged()); assertEquals(1, proof.other.recycled);
        proof.other=null; rejects(() -> proof.assertOtherUnchanged());
        System.out.println("PASS fixture/proof/engine host contract checks: " + checks);
    }
}
'''
    for key, value in {"MATCHES": matches, "SENSITIVE": sensitive, "CONFIGURE": configure,
                       "READBACK": readback, "OTHER_READBACK": other_readback, "BEFORE": before}.items():
        harness = harness.replace("@" + key + "@", value)
    env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C"}
    with tempfile.TemporaryDirectory(prefix="fixture-contract-host-") as directory:
        work = Path(directory)
        source = work / "FixtureContractTest.java"
        source.write_text(harness)
        command([args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main", "-encoding", "UTF-8", "-d", work, source], env, 30)
        output = command([args.java, "-cp", work, "FixtureContractTest"], env, 10)
        if not output.startswith("PASS fixture/proof/engine host contract checks: ") or len(output) > 100:
            raise RuntimeError("invalid fixture contract result")
        print(output.strip())


if __name__ == "__main__":
    main()
