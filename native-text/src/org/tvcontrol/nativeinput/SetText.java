package org.tvcontrol.nativeinput;

import android.os.Bundle;
import android.view.accessibility.AccessibilityNodeInfo;
import com.android.uiautomator.core.UiObject;
import com.android.uiautomator.core.UiSelector;
import com.android.uiautomator.testrunner.UiAutomatorTestCase;
import java.io.File;

/** Deprecated legacy research wrapper only. Not included in modern helper APK. */
public final class SetText extends UiAutomatorTestCase {
    @Override protected void setUp() throws Exception {
        if (!"false".equals(getParams().getString("disable_ime")))
            throw new IllegalStateException("native_text_unsafe_runner_parameters");
        super.setUp();
    }
    private static final class Focused extends UiObject {
        Focused() { super(new UiSelector().focused(true)); }
        AccessibilityNodeInfo node() { return findAccessibilityNodeInfo(0); }
    }
    public void testSetText() {
        TextEngine.runOnce(new File("/data/local/tmp"), getParams().getString("token"),
            new TextEngine.FocusSource() { public AccessibilityNodeInfo focused() { return new Focused().node(); } },
            new TextEngine.ResultSink() { public void status(String value) {
                Bundle result = new Bundle(); result.putString("tv_native_text_v1", value);
                getAutomationSupport().sendStatus(0, result);
            }});
    }
}
