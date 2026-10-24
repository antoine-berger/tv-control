package org.tvcontrol.nativeinput;

import android.accessibilityservice.AccessibilityServiceInfo;
import android.app.Instrumentation;
import android.app.UiAutomation;
import android.os.Bundle;
import android.view.accessibility.AccessibilityNodeInfo;

/** No UI/network/service/IME changes. Installation requires separate explicit approval. */
public class NativeTextInstrumentation extends Instrumentation {
    protected Bundle arguments;
    @Override public void onCreate(Bundle args) {
        arguments = args == null ? new Bundle() : args;
        super.onCreate(args);
        start();
    }
    static AccessibilityNodeInfo inputFocus(UiAutomation automation) {
        AccessibilityNodeInfo root = automation.getRootInActiveWindow();
        if (root == null) return null;
        try { return root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT); }
        finally { root.recycle(); }
    }
    protected UiAutomation connectForText() {
        UiAutomation automation = getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES);
        if (automation == null) return null;
        AccessibilityServiceInfo info = automation.getServiceInfo();
        if (info == null) return null;
        // Configure only this temporary automation connection, never secure settings.
        info.flags |= AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS;
        automation.setServiceInfo(info);
        return automation;
    }
    @Override public void onStart() {
        final Bundle result = new Bundle();
        result.putString("tv_native_text_v1", "refused");
        try {
            final UiAutomation automation = connectForText();
            if (automation != null) {
                TextEngine.runOnce(getTargetContext().getCacheDir(), arguments.getString("token"),
                    new TextEngine.FocusSource() { public AccessibilityNodeInfo focused() { return inputFocus(automation); } },
                    new TextEngine.ResultSink() { public void status(String status) { result.putString("tv_native_text_v1", status); } });
            }
        } catch (Exception ignored) {
            // Connection failures are bounded by host deadline. No exception/private UI logging.
            result.putString("tv_native_text_v1", "uncertain");
        } finally { finish(0, result); }
    }
}
