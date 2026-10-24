package org.tvcontrol.nativeinput;

import android.app.UiAutomation;

/** Lab entrypoint: inherits the real common onStart and Instrumentation.finish. */
public final class LabSocketInstrumentation extends NativeTextInstrumentation {
    @Override protected boolean prepareRequest(UiAutomation automation,NativePendingRequest pending) throws Exception {
        if(arguments==null || arguments.keySet().size()!=1 || !arguments.containsKey("lab_instance"))return false;
        String instance=arguments.getString("lab_instance");
        if(instance==null || !instance.matches("[0-9a-f]{32}"))return false;
        pending.publicInstance(instance);
        // Creation is pure. Register the one owner before any resource open or ingress I/O.
        LabSocketSession session=LabAndroidIngress.create(automation,pending,this,instance);
        pending.attach(session);
        return session.prepare();
    }
}
