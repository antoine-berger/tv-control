package org.tvcontrol.nativeinput;

import android.graphics.Rect;
import android.os.Bundle;
import android.os.SystemClock;
import android.view.accessibility.AccessibilityNodeInfo;
import android.app.UiAutomation;
import java.util.List;
import java.io.DataOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.UUID;

/** Test-only runner for a fresh owned emulator; never part of helper distribution. */
public final class RuntimeProof extends NativeTextInstrumentation {
    private static final String ID = "org.tvcontrol.fixture:id/editor";
    private UiAutomation automation;
    private AccessibilityNodeInfo editor(String id) {
        AccessibilityNodeInfo root = automation.getRootInActiveWindow();
        if (root == null) return null;
        try {
            List<AccessibilityNodeInfo> nodes = root.findAccessibilityNodeInfosByViewId(id);
            if (nodes.size() == 1) return nodes.get(0);
            for (AccessibilityNodeInfo n : nodes) n.recycle();
            return null;
        } finally { root.recycle(); }
    }
    private static void assertTrue(String reason, boolean value) { if (!value) throw new AssertionError(reason); }
    private static void assertTrue(boolean value) { assertTrue("assertion", value); }
    private static void assertFalse(String reason, boolean value) { assertTrue(reason, !value); }
    private static void assertNotNull(String reason, Object value) { assertTrue(reason, value != null); }
    private static void assertEquals(Object expected, Object actual) {
        assertTrue("equality", expected == null ? actual == null : expected.equals(actual));
    }
    @Override public void onStart() {
        Bundle result = new Bundle();
        result.putString("tv_native_runtime_proof", "failed");
        try {
            automation = connectForText();
            assertNotNull("automation", automation);
            automation.waitForIdle(200, 2000);
            testFixture();
            result.putString("tv_native_runtime_proof", "passed");
        } catch (Exception | AssertionError ignored) {
            // Fixed result only, no private UI contents or exception text.
        } finally { finish(0, result); }
    }
    private static void field(DataOutputStream out, String s) throws Exception {
        byte[] b = s.getBytes(StandardCharsets.UTF_8); out.writeInt(b.length); out.write(b);
    }
    public void testFixture() throws Exception {
        String mode = arguments.getString("mode");
        boolean insertion = "normal".equals(mode);
        AccessibilityNodeInfo n = editor(ID);
        assertNotNull("fixture editor absent", n);
        String before = n.getText() == null ? "" : n.getText().toString();
        if ("key-remap".equals(mode)) {
            assertEquals("Tqnyq", before);
            n.recycle();
            return;
        }
        String after = before + "Tanya é🙂%s'";
        String token = UUID.randomUUID().toString().replace("-", "");
        File dir = new File(getTargetContext().getCacheDir().getCanonicalFile(), "tv-native-text-" + token);
        assertTrue("exclusive fixture directory", dir.mkdir());
        dir.setReadable(false, false); dir.setWritable(false, false); dir.setExecutable(false, false);
        assertTrue(dir.setReadable(true, true)); assertTrue(dir.setWritable(true, true)); assertTrue(dir.setExecutable(true, true));
        File request = new File(dir, "request");
        try {
            Rect bounds = new Rect(); n.getBoundsInScreen(bounds);
            try (DataOutputStream out = new DataOutputStream(new FileOutputStream(request))) {
                out.writeInt(0x54564e31); out.writeLong(SystemClock.uptimeMillis() + 4000);
                field(out, n.getPackageName().toString()); field(out, n.getViewIdResourceName());
                field(out, n.getClassName().toString());
                out.writeInt("stale-window".equals(mode) ? n.getWindowId() + 1 : n.getWindowId());
                out.writeInt(bounds.left); out.writeInt(bounds.top); out.writeInt(bounds.right); out.writeInt(bounds.bottom);
                field(out, before); field(out, after);
            }
            request.setReadable(false, false); request.setWritable(false, false);
            assertTrue(request.setReadable(true, true)); assertTrue(request.setWritable(true, true));
            if ("focus".equals(mode)) {
                AccessibilityNodeInfo other = editor("org.tvcontrol.fixture:id/other");
                assertNotNull("other editor", other);
                try { assertTrue(other.performAction(AccessibilityNodeInfo.ACTION_FOCUS)); }
                finally { other.recycle(); }
            }
            if ("cancel".equals(mode)) assertTrue(new File(dir, "cancel").createNewFile());
            final String[] status = new String[1];
            TextEngine.ResultSink sink = new TextEngine.ResultSink() {
                public void status(String result) {
                    if (status[0] != null) throw new AssertionError("duplicate status");
                    status[0] = result;
                }
            };
            TextEngine.runOnce(getTargetContext().getCacheDir(), token,
                new TextEngine.FocusSource() { public AccessibilityNodeInfo focused() { return inputFocus(automation); } }, sink);
            assertEquals(insertion ? "verified" : "refused", status[0]);
            assertTrue(n.refresh());
            assertEquals(insertion ? after : before, n.getText() == null ? "" : n.getText().toString());
            assertFalse("request consumed", request.exists());
            status[0] = null;
            TextEngine.runOnce(getTargetContext().getCacheDir(), token,
                new TextEngine.FocusSource() { public AccessibilityNodeInfo focused() { return inputFocus(automation); } }, sink);
            assertEquals("refused", status[0]);
            assertTrue(n.refresh());
            assertEquals(insertion ? after : before, n.getText() == null ? "" : n.getText().toString());
        } finally {
            n.recycle();
            if (request.exists()) assertTrue(request.delete());
            File cancel = new File(dir, "cancel"); if (cancel.exists()) assertTrue(cancel.delete());
            assertTrue("owned cleanup", dir.delete());
        }
    }
}
