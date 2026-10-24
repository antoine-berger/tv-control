#!/usr/bin/env python3
"""Compile actual helper Request decoder against a Rect value fake; no Android/device."""
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
    start = source.index("    static final class Request {")
    end = source.index("        boolean live(", start)
    # Keep the actual fields, constructor and strict UTF-8 decoder unchanged;
    # omit only Android-dependent matching/liveness methods from this host test.
    decoder = source[start:end] + "    }\n"
    fixture = (root / "tests/protocol_decoder.java.in").read_text().replace("@REQUEST_DECODER@", decoder)
    with tempfile.TemporaryDirectory(prefix="tv-native-protocol-") as temporary:
        java = Path(temporary) / "ProtocolDecoder.java"
        java.write_text(fixture)
        subprocess.run([args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main",
                        "-d", temporary, str(java)], check=True)
        subprocess.run([args.java, "-cp", temporary, "ProtocolDecoder",
                        str(root / "tests/protocol_golden.hex")], check=True)


if __name__ == "__main__":
    main()
