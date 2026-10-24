#!/usr/bin/env python3
"""Real session lifecycle with fake ports/cache/editor; no Android or SDK execution."""
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
    parser.add_argument("--java", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent
    core = root / "src/org/tvcontrol/nativeinput"
    modern = root.parent.parent / "modern/src/org/tvcontrol/nativeinput"
    env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C"}
    with tempfile.TemporaryDirectory(prefix="lab-session-host-") as directory:
        work = Path(directory)
        stub = work / "org/tvcontrol/nativeinput/LabSocketChannel.java"
        stub.parent.mkdir(parents=True)
        stub.write_text("package org.tvcontrol.nativeinput; final class LabSocketChannel { interface StopSignal { boolean stopped(); } }")
        sources = [core / name for name in ("LabProtocol.java", "EditorBinding.java", "IssuedEditor.java",
                   "LabReplies.java", "LabSocketSession.java")]
        sources += [stub, modern / "NativePendingRequest.java", root / "tests/SocketSessionTest.java"]
        command([args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main", "-source", "8", "-target", "8",
                 "-Xlint:-options", "-d", work, *sources], env, 30)
        result = command([args.java, "-cp", work, "org.tvcontrol.nativeinput.SocketSessionTest"], env, 15)
        if not re.fullmatch(r"PASS actual lab session host checks: [0-9]+\n", result):
            raise RuntimeError("invalid session host result")
        print(result.strip())


if __name__ == "__main__":
    main()
