package org.tvcontrol.nativeinput;

import android.accessibilityservice.AccessibilityServiceInfo;
import android.app.UiAutomation;
import android.graphics.Rect;
import android.view.accessibility.AccessibilityNodeInfo;

/** Lab-only observation issuer. No action, focus repair, transport or payload output. */
final class LabEditorIssuer {
    static final class Capture {
        final EditorBinding binding;
        final long observedAt;
        private Capture(EditorBinding value, long time) { binding = value; observedAt = time; }
        @Override public String toString() { return "CapturedLabEditor"; }
    }
    private static LabProtocol.Fault unsupported() {
        return new LabProtocol.Fault(LabProtocol.Reason.EDITOR_UNSUPPORTED);
    }
    private static boolean safe(AccessibilityNodeInfo node, String pkg, String resource) {
        // Check secret/hint and intended target metadata BEFORE reading any text.
        return node != null && node.isFocused() && node.isEditable() && node.isEnabled()
            && node.isVisibleToUser() && !node.isPassword()
            && !TextEngine.sensitiveInputType(node.getInputType())
            && !node.isShowingHintText() && node.getHintText() == null
            && pkg.contentEquals(node.getPackageName() == null ? "" : node.getPackageName())
            && resource.equals(node.getViewIdResourceName())
            && "android.widget.EditText".contentEquals(node.getClassName() == null ? "" : node.getClassName())
            && (node.getActions() & AccessibilityNodeInfo.ACTION_SET_TEXT) != 0;
    }
    static Capture capture(UiAutomation automation, IssuedEditor.Clock clock,
            String expectedPackage, String expectedResource) throws LabProtocol.Fault {
        if (automation == null || clock == null || expectedPackage == null || expectedPackage.isEmpty()
                || expectedResource == null || expectedResource.isEmpty()) throw unsupported();
        long observedAt = clock.nowMillis();
        if (observedAt < 0) throw new LabProtocol.Fault(LabProtocol.Reason.DEADLINE);
        AccessibilityNodeInfo node = null;
        try {
            AccessibilityServiceInfo info = automation.getServiceInfo();
            if (info == null || (info.flags & AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS) == 0)
                throw unsupported();
            node = NativeTextInstrumentation.inputFocus(automation);
            if (!safe(node, expectedPackage, expectedResource) || !TextEngine.uniqueFocus(node)
                    || !node.refresh() || !safe(node, expectedPackage, expectedResource)) throw unsupported();
            CharSequence value = node.getText();
            if (value == null || value.length() > 256) throw unsupported();
            String before = value.toString();
            Rect bounds = new Rect(); node.getBoundsInScreen(bounds);
            EditorBinding binding = new EditorBinding(node.getPackageName().toString(),
                node.getViewIdResourceName(), node.getClassName().toString(), node.getWindowId(),
                bounds.left, bounds.top, bounds.right, bounds.bottom, before,
                node.getTextSelectionStart(), node.getTextSelectionEnd());
            return new Capture(binding, observedAt);
        } catch (LabProtocol.Fault failure) {
            throw failure;
        } catch (RuntimeException failure) {
            throw unsupported();
        } finally {
            if (node != null) node.recycle();
        }
    }
    private LabEditorIssuer() {}
}
