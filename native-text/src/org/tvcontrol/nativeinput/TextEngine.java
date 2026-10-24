package org.tvcontrol.nativeinput;

import android.graphics.Rect;
import android.os.Build;
import android.os.Bundle;
import android.os.SystemClock;
import android.text.InputType;
import android.view.accessibility.AccessibilityNodeInfo;
import java.io.DataInputStream;
import java.io.File;
import java.io.FileInputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayDeque;

/** Shared bounded engine: Android public APIs only; no transport or lifecycle ownership. */
public final class TextEngine {
    static final class Request {
        long expires;
        String pkg, resource, role, before, after;
        int window;
        Rect bounds;
        Request(DataInputStream in) throws Exception {
            if (in.readInt() != 0x54564e31) throw new Exception();
            expires = in.readLong();
            pkg = text(in); resource = text(in); role = text(in);
            window = in.readInt();
            bounds = new Rect(in.readInt(), in.readInt(), in.readInt(), in.readInt());
            before = text(in); after = text(in);
            if (in.read() != -1 || pkg.isEmpty() || resource.isEmpty() || role.isEmpty()
                    || before.length() > 256 || after.length() > 256
                    || !after.startsWith(before) || after.equals(before)) throw new Exception();
        }
        private static String text(DataInputStream in) throws Exception {
            int n = in.readInt();
            if (n < 0 || n > 2048) throw new Exception();
            byte[] bytes = new byte[n]; in.readFully(bytes);
            String value = new String(bytes, StandardCharsets.UTF_8);
            if (!java.util.Arrays.equals(bytes, value.getBytes(StandardCharsets.UTF_8)))
                throw new Exception();
            return value;
        }
        boolean live(File dir) {
            long left = expires - SystemClock.uptimeMillis();
            return left > 0 && left <= 5000 && !new File(dir, "cancel").exists();
        }
        boolean matches(AccessibilityNodeInfo n, String expected) {
            if (n == null || !n.isFocused() || !n.isEditable() || !n.isEnabled()
                    || !n.isVisibleToUser() || n.isPassword() || sensitiveInputType(n.getInputType())
                    || n.isShowingHintText()
                    || n.getHintText() != null || n.getText() == null) return false;
            Rect actual = new Rect(); n.getBoundsInScreen(actual);
            return pkg.contentEquals(n.getPackageName() == null ? "" : n.getPackageName())
                && resource.equals(n.getViewIdResourceName())
                && role.contentEquals(n.getClassName() == null ? "" : n.getClassName())
                && window == n.getWindowId() && bounds.equals(actual)
                && expected.contentEquals(n.getText());
        }
    }

    static boolean sensitiveInputType(int inputType) {
        int kind = inputType & InputType.TYPE_MASK_CLASS;
        int variation = inputType & InputType.TYPE_MASK_VARIATION;
        return (kind == InputType.TYPE_CLASS_TEXT
                && (variation == InputType.TYPE_TEXT_VARIATION_PASSWORD
                    || variation == InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
                    || variation == InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD))
            || (kind == InputType.TYPE_CLASS_NUMBER
                && variation == InputType.TYPE_NUMBER_VARIATION_PASSWORD);
    }

    // Inspect the accessible tree only; reject partial/oversized trees rather than guess.
    static boolean uniqueFocus(AccessibilityNodeInfo selected) {
        AccessibilityNodeInfo root = AccessibilityNodeInfo.obtain(selected);
        ArrayDeque<AccessibilityNodeInfo> pending = new ArrayDeque<AccessibilityNodeInfo>();
        try {
            for (int depth = 0; ; depth++) {
                if (depth >= 64) return false;
                AccessibilityNodeInfo parent = root.getParent();
                if (parent == null) break;
                root.recycle(); root = parent;
            }
            pending.add(root); root = null;
            int visited = 0, focused = 0;
            while (!pending.isEmpty()) {
                AccessibilityNodeInfo n = pending.remove();
                try {
                    if (++visited > 1024 || n.getChildCount() > 1024) return false;
                    if (n.isFocused() && (!n.equals(selected) || ++focused > 1)) return false;
                    for (int i = 0; i < n.getChildCount(); i++) {
                        AccessibilityNodeInfo child = n.getChild(i);
                        if (child == null || pending.size() >= 1024) {
                            if (child != null) child.recycle();
                            return false;
                        }
                        pending.add(child);
                    }
                } finally { n.recycle(); }
            }
            return focused == 1;
        } finally {
            if (root != null) root.recycle();
            while (!pending.isEmpty()) pending.remove().recycle();
        }
    }

    static boolean readyToDispatch(AccessibilityNodeInfo node, Request request) {
        // Traversal can be slow and change focus: refresh after it, not only before.
        return node != null && node.refresh() && request.matches(node, request.before)
            && node.getTextSelectionStart() == request.before.length()
            && node.getTextSelectionEnd() == request.before.length()
            && (node.getActions() & AccessibilityNodeInfo.ACTION_SET_TEXT) != 0;
    }

    public interface FocusSource { AccessibilityNodeInfo focused(); }
    public interface ResultSink { void status(String value); }

    public static void runOnce(File base, String token, FocusSource source, ResultSink output) {
        String status = "refused";
        boolean dispatched = false;
        AccessibilityNodeInfo node = null;
        try {
            // API 26 is required for public hint detection. Do not guess on older TVs.
            if (Build.VERSION.SDK_INT < 26) return;
            if (token == null || !token.matches("[0-9a-f]{32}")) return;
            File dir = new File(base.getCanonicalFile(), "tv-native-text-" + token);
            File input = new File(dir, "request");
            if (!input.getCanonicalPath().equals(input.getAbsolutePath())
                    || !input.isFile() || input.length() > 16384) return;
            Request request;
            try (DataInputStream in = new DataInputStream(new FileInputStream(input))) {
                request = new Request(in);
            }
            // Consume before any mutation. A timeout can never authorize replay.
            if (!input.delete() || !request.live(dir)) return;
            node = source.focused();
            if (!request.matches(node, request.before) || !uniqueFocus(node)
                    || node.getTextSelectionStart() != request.before.length()
                    || node.getTextSelectionEnd() != request.before.length()
                    || (node.getActions() & AccessibilityNodeInfo.ACTION_SET_TEXT) == 0) return;
            Bundle arguments = new Bundle();
            arguments.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,
                    request.after);
            if (!readyToDispatch(node, request) || !request.live(dir)) return;
            // No public API makes this final check and performAction atomic.
            dispatched = true;
            boolean accepted = node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, arguments);
            status = "uncertain";
            if (accepted && request.live(dir) && node.refresh()
                    && request.matches(node, request.after) && uniqueFocus(node)
                    && request.live(dir)) status = "verified";
        } catch (Exception ignored) {
            status = dispatched ? "uncertain" : "refused";
        } finally {
            if (node != null) node.recycle();
            output.status(status);
        }
    }
}
