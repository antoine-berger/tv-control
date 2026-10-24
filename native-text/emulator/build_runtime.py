#!/usr/bin/env python3
"""Build owned fixture/helper/proof APKs; no Gradle/download/device actions."""
import argparse
import hashlib
from pathlib import Path
import subprocess
import zipfile


def run(args):
    subprocess.run([str(x) for x in args], check=True, timeout=120)


def dex(java, tools, platform, sources, output):
    classes = output / "classes"
    classes.mkdir(parents=True)
    run([java, "-m", "jdk.compiler/com.sun.tools.javac.Main", "-source", "8", "-target", "8",
         "-Xlint:-options", "-g:none", "-encoding", "UTF-8", "-bootclasspath", platform / "android.jar",
         "-d", classes, *sources])
    run([java, "-cp", tools / "lib/d8.jar", "com.android.tools.r8.D8", "--release",
         "--min-api", "26", "--lib", platform / "android.jar", "--output", output,
         *sorted(classes.rglob("*.class"))])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", required=True, type=Path)
    parser.add_argument("--java", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=False)
    root = Path(__file__).resolve().parent
    platform = args.sdk / "platforms/android-28"
    tools = args.sdk / "build-tools/35.0.0"
    run([args.java, "-version"])
    run([args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main", "-version"])
    # Runtime proof uses recorded preinstalled hosted JDK, not the pinned legacy build.
    key = args.out / "ephemeral-test-key.p12"
    run([args.java.parent / "keytool", "-genkeypair", "-keystore", key, "-storepass", "fixture-test-only",
         "-keypass", "fixture-test-only", "-alias", "fixture", "-dname", "CN=Disposable Emulator Fixture",
         "-keyalg", "RSA", "-keysize", "2048", "-validity", "2", "-noprompt"])
    def apk(name, manifest, sources, resources=None):
        work = args.out / name
        work.mkdir()
        generated = work / "generated"
        generated.mkdir()
        unsigned, aligned = work / "unsigned.apk", work / "aligned.apk"
        command = [tools / "aapt2", "link", "-o", unsigned, "--manifest", manifest,
                   "-I", platform / "android.jar", "--java", generated]
        if resources:
            resource = work / "resources.zip"
            run([tools / "aapt2", "compile", "--dir", resources, "-o", resource])
            command.append(resource)
        run(command)
        dex(args.java, tools, platform, sources + sorted(generated.rglob("*.java")), work / "dex")
        data = (work / "dex/classes.dex").read_bytes()
        with zipfile.ZipFile(unsigned, "a") as archive:
            entry = zipfile.ZipInfo("classes.dex", date_time=(1980, 1, 1, 0, 0, 0))
            entry.external_attr = 0o100644 << 16
            archive.writestr(entry, data)
        run([tools / "zipalign", "-f", "4", unsigned, aligned])
        output = args.out / (name + ".apk")
        run([args.java, "-jar", tools / "lib/apksigner.jar", "sign", "--ks", key,
             "--ks-pass", "pass:fixture-test-only", "--out", output, aligned])
        print(name, "classes.dex sha256", hashlib.sha256(data).hexdigest())
        print(name, "APK sha256", hashlib.sha256(output.read_bytes()).hexdigest())
    try:
        apk("fixture", root / "fixture/AndroidManifest.xml",
            sorted((root / "fixture/src").rglob("*.java")), root / "fixture/res")
        shared = [root.parent / "src/org/tvcontrol/nativeinput/TextEngine.java"]
        modern = sorted((root.parent / "modern/src").rglob("*.java"))
        apk("helper", root.parent / "modern/AndroidManifest.xml", shared + modern)
        apk("proof", root / "ProofManifest.xml", shared + modern + sorted((root / "src").rglob("*.java")))
    finally:
        if key.exists():
            key.unlink()


if __name__ == "__main__":
    main()
