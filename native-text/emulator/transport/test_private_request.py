#!/usr/bin/env python3
"""Actual private scratch owner/engine decoder on host temp files; no Android/SDK."""
import argparse
import os
from pathlib import Path
import re
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from owned_runtime import command


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--java", required=True, type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent
    engine = (root.parent.parent / "src/org/tvcontrol/nativeinput/TextEngine.java").read_text()
    start = engine.index("    static final class Request {")
    end = engine.index("        boolean live(", start)
    decoder = engine[start:end] + "    }\n"
    probe = """package org.tvcontrol.nativeinput;
import java.io.DataInputStream;
import java.nio.charset.StandardCharsets;
final class RequestDecoderProbe {
    static final class Rect {
        final int left,top,right,bottom;
        Rect(int a,int b,int c,int d) { left=a;top=b;right=c;bottom=d; }
    }
""" + decoder + "}\n"
    env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C"}
    with tempfile.TemporaryDirectory(prefix="lab-private-request-compile-") as directory:
        work = Path(directory)
        decoder_file = work / "RequestDecoderProbe.java"
        decoder_file.write_text(probe)
        src = root / "src/org/tvcontrol/nativeinput"
        sources = [src / name for name in ("LabProtocol.java", "EditorBinding.java", "IssuedEditor.java", "LabPrivateRequest.java")]
        sources += [decoder_file, root / "tests/PrivateRequestTest.java"]
        command([args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main", "-encoding", "UTF-8",
                 "-source", "8", "-target", "8", "-Xlint:-options", "-d", work, *sources], env, 30)
        output = command([args.java, "-Djava.io.tmpdir=" + str(work), "-cp", work,
                          "org.tvcontrol.nativeinput.PrivateRequestTest"], env, 10)
        if not re.fullmatch(r"PASS private request host checks: [0-9]+\n", output):
            raise RuntimeError("invalid private request test result")
        print(output.strip())


if __name__ == "__main__":
    main()
