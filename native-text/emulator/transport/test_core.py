#!/usr/bin/env python3
"""Compile and run pure Java transport contracts only; no Android/SDK/socket use."""
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
    env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C"}
    with tempfile.TemporaryDirectory(prefix="lab-transport-core-") as directory:
        work = Path(directory)
        core = root / "src/org/tvcontrol/nativeinput"
        sources = [core / name for name in ("LabProtocol.java", "EditorBinding.java", "IssuedEditor.java")]
        sources += [root / "tests/TransportCoreTest.java"]
        command([args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main", "-encoding", "UTF-8",
                 "-source", "8", "-target", "8", "-Xlint:-options", "-d", work, *sources], env, 30)
        result = command([args.java, "-cp", work, "org.tvcontrol.nativeinput.TransportCoreTest"], env, 10)
        if not re.fullmatch(r"PASS lab pure transport core checks: [0-9]+\n", result):
            raise RuntimeError("invalid pure transport test result")
        print(result.strip())


if __name__ == "__main__":
    main()
