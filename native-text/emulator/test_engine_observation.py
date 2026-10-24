#!/usr/bin/env python3
"""Exercise actual diagnostic Java with a host JDK; no Android APIs or SDK tools."""
import argparse
import os
from pathlib import Path
import tempfile

from owned_runtime import command


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--java", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).parent
    harness = r'''
package org.tvcontrol.nativeinput;
public final class EngineObservationTest {
    static int checks;
    static void equal(String expected, String actual) {
        checks++; if (!expected.equals(actual)) throw new AssertionError("fixed diagnostic mismatch");
    }
    public static void main(String[] args) {
        EngineObservation value = new EngineObservation();
        equal("NOT_REPORTED", value.initial()); equal("NOT_REPORTED", value.replay());
        String[][] cases = {{"verified", "VERIFIED"}, {"refused", "REFUSED"},
            {"uncertain", "UNCERTAIN"}, {"", "INVALID"},
            {"fixture-private-value", "INVALID"}, {"VERIFIED", "INVALID"}, {null, "INVALID"}};
        for (String[] pair : cases) {
            value = new EngineObservation();
            value.observe(pair[0], false, false);
            equal(pair[1], value.initial()); equal("NOT_REPORTED", value.replay());
            value.observe(pair[0], true, false);
            equal(pair[1], value.initial()); equal(pair[1], value.replay());
            value.observe("fixture-private-value", true, true);
            equal("MULTIPLE", value.replay()); equal(pair[1], value.initial());
            value.observe(null, false, true);
            equal("MULTIPLE", value.initial()); equal("MULTIPLE", value.replay());
        }
        System.out.println("PASS fixed engine observation checks: " + checks);
    }
}
'''
    env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C"}
    with tempfile.TemporaryDirectory(prefix="engine-observation-host-") as directory:
        work = Path(directory)
        source = work / "EngineObservationTest.java"
        source.write_text(harness)
        command([args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main", "-d", work,
                 root / "src/org/tvcontrol/nativeinput/EngineObservation.java", source], env, 30)
        output = command([args.java, "-cp", work, "org.tvcontrol.nativeinput.EngineObservationTest"], env, 10)
        if output != "PASS fixed engine observation checks: 58\n":
            raise RuntimeError("invalid host observation test result")
        print(output.strip())


if __name__ == "__main__":
    main()
