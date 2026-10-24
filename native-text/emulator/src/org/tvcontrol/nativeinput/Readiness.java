package org.tvcontrol.nativeinput;

/** Test-only bounded read-only polling. No input/action/retry of a mutation. */
final class Readiness {
    interface Probe { boolean ready(); }
    interface Clock { long now(); void sleep(long millis); }
    static boolean await(Probe probe, Clock clock) {
        final long start = clock.now();
        for (int attempt = 0; attempt < 100; attempt++) {
            long elapsed = clock.now() - start;
            if (elapsed < 0 || elapsed >= 20000) return false;
            if (probe.ready()) {
                long finished = clock.now() - start;
                return finished >= 0 && finished < 20000;
            }
            long remaining = 20000 - (clock.now() - start);
            if (remaining <= 0) return false;
            clock.sleep(Math.min(200, remaining));
        }
        return false;
    }
}
