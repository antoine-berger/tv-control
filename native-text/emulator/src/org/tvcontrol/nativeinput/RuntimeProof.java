package org.tvcontrol.nativeinput;

import android.graphics.Rect;
import android.content.Context;
import android.app.KeyguardManager;
import android.os.PowerManager;
import android.accessibilityservice.AccessibilityServiceInfo;
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
    enum Stage { CONNECT_AUTOMATION, WAIT_IDLE, ROOT_LOOKUP, EDITOR_LOOKUP,
        READ_EDITOR, REMAP_CHECK, CREATE_REQUEST, SERIALIZE_REQUEST, FOCUS_SWITCH,
        CANCEL_MARKER, ENGINE_INVOKE, ENGINE_STATUS, READBACK_REFRESH, READBACK_CHECK,
        REQUEST_CONSUMED, REPLAY_INVOKE, REPLAY_STATUS, REPLAY_READBACK, CLEANUP, DONE }
    enum ErrorCode { NONE, ASSERTION, EXCEPTION }
    enum SyntheticReadback { NOT_CHECKED, EXPECTED_REMAP, UNCHANGED_LITERAL, EMPTY, OTHER }
    enum RootCategory { NOT_CHECKED, FIXTURE, HELPER, IME, SYSTEM, OTHER, MISSING }
    enum Overlay { NOT_CHECKED, ANR_DIALOG, CRASH_DIALOG, ERROR_DIALOG, GENERIC_PROMPT, UNKNOWN_SYSTEM, NON_SYSTEM }
    private Overlay overlay = Overlay.NOT_CHECKED;
    enum MatchCount { NOT_CHECKED, ZERO, ONE, MULTIPLE }
    enum ObservedState { UNKNOWN, YES, NO }
    private RootCategory rootCategory = RootCategory.NOT_CHECKED;
    private MatchCount matches = MatchCount.NOT_CHECKED;
    private ObservedState reportIds = ObservedState.UNKNOWN;
    private ObservedState interactive = ObservedState.UNKNOWN;
    private ObservedState keyguardLocked = ObservedState.UNKNOWN;
    private static ObservedState observed(boolean value) { return value ? ObservedState.YES : ObservedState.NO; }
    private void observeDeviceState() {
        // Read-only public APIs, diagnostic only; never changes proof decisions.
        try {
            PowerManager power = (PowerManager)getTargetContext().getSystemService(Context.POWER_SERVICE);
            if (power != null) interactive = observed(power.isInteractive());
            KeyguardManager keyguard = (KeyguardManager)getTargetContext().getSystemService(Context.KEYGUARD_SERVICE);
            if (keyguard != null) keyguardLocked = observed(keyguard.isKeyguardLocked());
            AccessibilityServiceInfo info = automation.getServiceInfo();
            if (info != null) reportIds = observed((info.flags & AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS) != 0);
        } catch (RuntimeException ignored) { /* Unavailable diagnostics stay UNKNOWN. */ }
    }
    private static RootCategory category(CharSequence name) {
        if (name == null) return RootCategory.MISSING;
        String value = name.toString();
        if ("org.tvcontrol.fixture".equals(value)) return RootCategory.FIXTURE;
        if ("org.tvcontrol.nativehelper".equals(value)) return RootCategory.HELPER;
        if ("com.android.inputmethod.latin".equals(value)) return RootCategory.IME;
        if ("android".equals(value) || "com.android.systemui".equals(value)) return RootCategory.SYSTEM;
        return RootCategory.OTHER;
    }
    private Stage stage = Stage.CONNECT_AUTOMATION;
    private SyntheticReadback syntheticReadback = SyntheticReadback.NOT_CHECKED;
    private final EngineObservation engineObservation = new EngineObservation();
    private UiAutomation automation;
    private static boolean hasId(AccessibilityNodeInfo root, String id) {
        List<AccessibilityNodeInfo> nodes = root.findAccessibilityNodeInfosByViewId(id);
        boolean found = !nodes.isEmpty();
        for (AccessibilityNodeInfo node : nodes) node.recycle();
        return found;
    }
    private static Overlay classifyOverlay(AccessibilityNodeInfo root, RootCategory category) {
        if (category != RootCategory.SYSTEM) return Overlay.NON_SYSTEM;
        // Known AOSP resource IDs only; do not read labels or dismiss anything.
        if (hasId(root, "android:id/aerr_wait")) return Overlay.ANR_DIALOG;
        if (hasId(root, "android:id/aerr_restart") || hasId(root, "android:id/aerr_app_info")) return Overlay.CRASH_DIALOG;
        if (hasId(root, "android:id/aerr_close")) return Overlay.ERROR_DIALOG;
        if (hasId(root, "android:id/alertTitle") || hasId(root, "android:id/button1")) return Overlay.GENERIC_PROMPT;
        return Overlay.UNKNOWN_SYSTEM;
    }
    private AccessibilityNodeInfo editor(String id) {
        stage = Stage.ROOT_LOOKUP;
        matches = MatchCount.NOT_CHECKED;
        overlay = Overlay.NOT_CHECKED;
        AccessibilityNodeInfo root = automation.getRootInActiveWindow();
        if (root == null) { rootCategory = RootCategory.MISSING; return null; }
        try {
            rootCategory = category(root.getPackageName());
            overlay = classifyOverlay(root, rootCategory);
            stage = Stage.EDITOR_LOOKUP;
            List<AccessibilityNodeInfo> nodes = root.findAccessibilityNodeInfosByViewId(id);
            matches = nodes.isEmpty() ? MatchCount.ZERO : nodes.size() == 1 ? MatchCount.ONE : MatchCount.MULTIPLE;
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
    private static void assertEditorText(AccessibilityNodeInfo node, String expected, boolean expectNull) {
        CharSequence value = node.getText();
        if (expectNull) assertTrue("empty fixture remains null", value == null);
        else { assertNotNull("fixture text is observable", value); assertEquals(expected, value.toString()); }
    }
    @Override public void onStart() {
        Bundle result = new Bundle();
        result.putString("tv_native_runtime_proof", "failed");
        ErrorCode error = ErrorCode.NONE;
        try {
            automation = connectForText();
            assertNotNull("automation", automation);
            stage = Stage.WAIT_IDLE;
            automation.waitForIdle(200, 2000);
            observeDeviceState();
            testFixture();
            stage = Stage.DONE;
            result.putString("tv_native_runtime_proof", "passed");
        } catch (AssertionError ignored) {
            error = ErrorCode.ASSERTION;
        } catch (Exception ignored) {
            error = ErrorCode.EXCEPTION;
        } finally {
            // Fixed enums only: never include UI text, exception messages or stacks.
            result.putString("tv_native_runtime_stage", stage.name());
            result.putString("tv_native_runtime_error", error.name());
            result.putString("tv_native_runtime_readback", syntheticReadback.name());
            result.putString("tv_native_runtime_overlay", overlay.name());
            result.putString("tv_native_runtime_root", rootCategory.name());
            result.putString("tv_native_runtime_matches", matches.name());
            result.putString("tv_native_runtime_report_ids", reportIds.name());
            result.putString("tv_native_runtime_interactive", interactive.name());
            result.putString("tv_native_runtime_keyguard_locked", keyguardLocked.name());
            result.putString("tv_native_runtime_engine_status", engineObservation.initial());
            result.putString("tv_native_runtime_replay_status", engineObservation.replay());
            finish(0, result);
        }
    }
    private static void field(DataOutputStream out, String s) throws Exception {
        byte[] b = s.getBytes(StandardCharsets.UTF_8); out.writeInt(b.length); out.write(b);
    }
    private boolean fixtureReady() {
        observeDeviceState();
        AccessibilityNodeInfo node = editor(ID);
        if (node == null) return false;
        try {
            Rect bounds = new Rect();
            node.getBoundsInScreen(bounds);
            return rootCategory == RootCategory.FIXTURE
                && "org.tvcontrol.fixture".contentEquals(node.getPackageName() == null ? "" : node.getPackageName())
                && ID.equals(node.getViewIdResourceName())
                && "android.widget.EditText".contentEquals(node.getClassName() == null ? "" : node.getClassName())
                && node.isFocused() && node.isEditable() && node.isEnabled()
                && node.isVisibleToUser() && !bounds.isEmpty();
        } finally { node.recycle(); }
    }
    public void testFixture() throws Exception {
        String mode = arguments.getString("mode");
        if ("ready".equals(mode)) {
            assertTrue("fixture accessibility readiness deadline", Readiness.await(
                new Readiness.Probe() { public boolean ready() { return fixtureReady(); } },
                new Readiness.Clock() {
                    public long now() { return SystemClock.uptimeMillis(); }
                    public void sleep(long millis) { SystemClock.sleep(millis); }
                }));
            return;
        }
        boolean insertion = "normal".equals(mode);
        AccessibilityNodeInfo n = editor(ID);
        assertNotNull("fixture editor absent", n);
        stage = Stage.READ_EDITOR;
        boolean emptyNull = "empty-null".equals(mode);
        CharSequence observedBefore = n.getText();
        if (emptyNull) assertTrue("empty fixture exposes null", observedBefore == null);
        else assertNotNull("fixture text is observable", observedBefore);
        String before = observedBefore == null ? "" : observedBefore.toString();
        if ("key-remap".equals(mode)) {
            stage = Stage.REMAP_CHECK;
            syntheticReadback = "Tqnyq".equals(before) ? SyntheticReadback.EXPECTED_REMAP
                : "Tanya".equals(before) ? SyntheticReadback.UNCHANGED_LITERAL
                : before.isEmpty() ? SyntheticReadback.EMPTY : SyntheticReadback.OTHER;
            assertEquals("Tqnyq", before);
            n.recycle();
            return;
        }
        if ("hint".equals(mode)) assertEquals("fixture hint", before);
        else if (!emptyNull && !"password".equals(mode) && !"visible-password".equals(mode))
            assertEquals("seed", before);
        if (!"hint".equals(mode) && !emptyNull) {
            int expectedSelection = "selection".equals(mode) ? 1 : before.length();
            assertEquals(expectedSelection, n.getTextSelectionStart());
            assertEquals(expectedSelection, n.getTextSelectionEnd());
        }
        String after = before + "Tanya é🙂%s'";
        stage = Stage.CREATE_REQUEST;
        String token = UUID.randomUUID().toString().replace("-", "");
        File dir = new File(getTargetContext().getCacheDir().getCanonicalFile(), "tv-native-text-" + token);
        assertTrue("exclusive fixture directory", dir.mkdir());
        dir.setReadable(false, false); dir.setWritable(false, false); dir.setExecutable(false, false);
        assertTrue(dir.setReadable(true, true)); assertTrue(dir.setWritable(true, true)); assertTrue(dir.setExecutable(true, true));
        File request = new File(dir, "request");
        try {
            stage = Stage.SERIALIZE_REQUEST;
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
                stage = Stage.FOCUS_SWITCH;
                try { assertTrue(other.performAction(AccessibilityNodeInfo.ACTION_FOCUS)); }
                finally { other.recycle(); }
            }
            if ("cancel".equals(mode)) {
                stage = Stage.CANCEL_MARKER;
                assertTrue(new File(dir, "cancel").createNewFile());
            }
            final String[] status = new String[1];
            final boolean[] replaying = new boolean[1];
            TextEngine.ResultSink sink = new TextEngine.ResultSink() {
                public void status(String result) {
                    engineObservation.observe(result, replaying[0], status[0] != null);
                    if (status[0] != null) throw new AssertionError("duplicate status");
                    status[0] = result;
                }
            };
            stage = Stage.ENGINE_INVOKE;
            TextEngine.runOnce(getTargetContext().getCacheDir(), token,
                new TextEngine.FocusSource() { public AccessibilityNodeInfo focused() { return inputFocus(automation); } }, sink);
            stage = Stage.ENGINE_STATUS;
            assertEquals(insertion ? "verified" : "refused", status[0]);
            stage = Stage.READBACK_REFRESH;
            assertTrue(n.refresh());
            stage = Stage.READBACK_CHECK;
            assertEditorText(n, insertion ? after : before, emptyNull);
            stage = Stage.REQUEST_CONSUMED;
            assertFalse("request consumed", request.exists());
            status[0] = null;
            replaying[0] = true;
            stage = Stage.REPLAY_INVOKE;
            TextEngine.runOnce(getTargetContext().getCacheDir(), token,
                new TextEngine.FocusSource() { public AccessibilityNodeInfo focused() { return inputFocus(automation); } }, sink);
            stage = Stage.REPLAY_STATUS;
            assertEquals("refused", status[0]);
            stage = Stage.REPLAY_READBACK;
            assertTrue(n.refresh());
            assertEditorText(n, insertion ? after : before, emptyNull);
        } finally {
            Stage previous = stage;
            stage = Stage.CLEANUP;
            n.recycle();
            if (request.exists()) assertTrue(request.delete());
            File cancel = new File(dir, "cancel"); if (cancel.exists()) assertTrue(cancel.delete());
            assertTrue("owned cleanup", dir.delete());
            stage = previous;
        }
    }
}
