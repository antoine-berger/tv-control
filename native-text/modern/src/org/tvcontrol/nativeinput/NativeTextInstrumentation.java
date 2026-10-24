package org.tvcontrol.nativeinput;

import android.accessibilityservice.AccessibilityServiceInfo;
import android.app.Instrumentation;
import android.app.UiAutomation;
import android.os.Bundle;
import android.os.SystemClock;
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
    /** Narrow ingress seam; attach its resource owner before opening anything. */
    protected boolean prepareRequest(UiAutomation automation, NativePendingRequest pending) throws Exception {
        // Existing private-cache research entrypoint. Lab ingress will override
        // this seam and must never take text or request tokens from arguments.
        pending.token(arguments.getString("token"));
        return true;
    }
    @Override public void onStart() {
        final Bundle result = new Bundle();
        // This resource owner exists even if context, connection or ingress throws.
        final NativePendingRequest pending = new NativePendingRequest();
        try {
            pending.startedUptime(SystemClock.uptimeMillis());
            pending.cache(getTargetContext().getCacheDir());
            final UiAutomation automation = connectForText();
            if (automation != null && prepareRequest(automation, pending) && pending.enter()) {
                TextEngine.runOnce(pending.cache(), pending.token(),
                    new TextEngine.FocusSource() { public AccessibilityNodeInfo focused() { return inputFocus(automation); } },
                    new TextEngine.ResultSink() { public void status(String status) { pending.result(status); } });
            }
        } catch (Throwable ignored) {
            // Connection failures are bounded by host deadline. No exception/private UI logging.
            pending.failed();
        } finally {
            NativePendingRequest.Terminal terminal = pending.finish();
            result.putString("tv_native_text_v1", terminal.status);
            result.putString("tv_native_cleanup_v1", terminal.cleanup.name());
            result.putString("tv_native_lab_protocol_v1", "1");
            result.putString("tv_native_entered_v1", terminal.entered ? "YES" : "NO");
            result.putString("tv_native_reason_v1", terminal.reason.name());
            if (terminal.publicInstance != null)
                result.putString("tv_native_lab_instance_v1", terminal.publicInstance);
            finish(0, result);
        }
    }
}
