#!/usr/bin/env python3
"""Run actual helper guard algorithms against Java fakes; no Android/device use."""
import argparse
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--java", required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    source = (root / "src/org/tvcontrol/nativeinput/TextEngine.java").read_text()
    start = source.index("    static boolean sensitiveInputType(")
    end = source.index("    public interface FocusSource", start)
    methods = source[start:end].replace("AccessibilityNodeInfo", "Node")
    fixture = (root / "tests/FocusGuards.java.in").read_text().replace("@METHODS@", methods)
    with tempfile.TemporaryDirectory(prefix="tv-native-guards-") as temporary:
        java = Path(temporary) / "FocusGuards.java"
        java.write_text(fixture)
        subprocess.run([args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main",
                        "-d", temporary, str(java)], check=True)
        subprocess.run([args.java, "-cp", temporary, "FocusGuards"], check=True)


if __name__ == "__main__":
    main()
