#!/usr/bin/env python3
"""Build only the reviewed lab APK with already provisioned API31 tools; no device actions."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import time
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from owned_runtime import command
from process_group import require_nonreaping_wait
from runtime_environment import (macho_arm64, private_directory, read_json, regular_owned,
                                 select_runtime, sha256, verify_local_inputs)

PACKAGE = "org.tvcontrol.nativehelper"
COMPONENT = PACKAGE + "/org.tvcontrol.nativeinput.LabSocketInstrumentation"
CERTIFICATE = re.compile(r"^Signer #1 certificate SHA-256 digest: ([0-9a-fA-F]{64})$", re.MULTILINE)


def checked_certificate(output):
    """Accept one signer/digest, never learn the pin from a device."""
    found = CERTIFICATE.findall(output)
    signer_ids = re.findall(r"^Signer #([0-9]+) certificate SHA-256 digest:", output, re.MULTILINE)
    if len(found) != 1 or signer_ids != ["1"]:
        raise RuntimeError("lab APK must have exactly one verified certificate")
    return found[0].lower()


def preflight(sdk, java, baseline, out, provenance):
    require_nonreaping_wait()
    if any(ord(char) < 32 or ord(char) == 127 for char in str(java)):
        raise RuntimeError("lab build Java path contains control characters")
    runtime = select_runtime("darwin-local", False)
    receipt = verify_local_inputs(sdk, baseline, out, provenance, runtime)
    lab = private_directory(sdk.parent)
    java = regular_owned(java, lab).resolve(strict=True)
    java_home = Path(receipt.get("java_home", "")).resolve(strict=True)
    if java != java_home / "bin/java" or not java_home.is_relative_to(lab):
        raise RuntimeError("lab build Java is not the approved private archive")
    java_receipt = receipt.get("java", {})
    files = java_receipt.get("files")
    if (java_receipt.get("verified") is not True or not isinstance(files, dict)
            or not 1 <= len(files) <= 4096 or not {"bin/java", "bin/keytool", "lib/modules"} <= files.keys()):
        raise RuntimeError("lab build Java provenance is incomplete")
    deadline = time.monotonic() + 90
    for name, digest in files.items():
        relative = Path(name)
        if relative.is_absolute() or ".." in relative.parts or not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise RuntimeError("lab build Java digest syntax is invalid")
        if sha256(regular_owned(java_home / relative, java_home), deadline) != digest:
            raise RuntimeError("lab build Java changed since provisioning")
    if any(str(path.relative_to(java_home)) not in files for path in java_home.rglob("*") if path.is_file()):
        raise RuntimeError("lab build Java has unverified additional files")
    if not all(macho_arm64(java_home / binary) for binary in ("bin/java", "bin/keytool")):
        raise RuntimeError("lab build Java must run natively on ARM64")
    required = {"platforms/android-31/android.jar", "build-tools/35.0.0/aapt2",
                "build-tools/35.0.0/zipalign", "build-tools/35.0.0/lib/d8.jar",
                "build-tools/35.0.0/lib/apksigner.jar"}
    if not required <= receipt["files"].keys():
        raise RuntimeError("lab build requires verified API31 and build-tools35")
    for directory in (sdk / "build-tools/35.0.0", sdk / "platforms/android-31"):
        if any(str(path.relative_to(sdk)) not in receipt["files"] for path in directory.rglob("*") if path.is_file()):
            raise RuntimeError("lab build SDK tree contains unverified additional files")
    if not all(macho_arm64(sdk / name) for name in ("build-tools/35.0.0/aapt2", "build-tools/35.0.0/zipalign")):
        raise RuntimeError("lab build tools require native ARM64 support")
    return java


def build(sdk, java, baseline, out, provenance, source_commit):
    if not re.fullmatch(r"[0-9a-f]{40}", source_commit):
        raise RuntimeError("lab build requires the exact reviewed source commit")
    java = preflight(sdk, java, baseline, out, provenance)
    root = Path(__file__).resolve().parent
    env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C"}
    if command(["/usr/bin/git", "-C", root, "rev-parse", "HEAD"], env).strip() != source_commit:
        raise RuntimeError("lab build checkout differs from the reviewed source")
    if command(["/usr/bin/git", "-C", root, "status", "--porcelain", "--untracked-files=all"], env).strip():
        raise RuntimeError("lab build requires a clean reviewed checkout")
    os.umask(0o077)
    out.mkdir(mode=0o700, exist_ok=False)
    home, temporary = out / "home", out / "tmp"
    home.mkdir(mode=0o700)
    temporary.mkdir(mode=0o700)
    env.update(HOME=str(home), TMPDIR=str(temporary), JAVA_HOME=str(java.parent.parent))
    tools, platform = sdk / "build-tools/35.0.0", sdk / "platforms/android-31"
    key, public_certificate = out / "ephemeral-test-key.p12", out / "ephemeral-certificate.der"
    stage = "INITIALIZE"
    def run(argv):
        try:
            return command(argv, env, 120)
        except Exception:
            raise RuntimeError("lab build tool failed at " + stage) from None
    try:
        stage = "TEST_KEY"
        run([java.parent / "keytool", "-genkeypair", "-keystore", key, "-storepass", "fixture-test-only",
             "-keypass", "fixture-test-only", "-alias", "fixture", "-dname", "CN=Disposable Native Text Lab",
             "-keyalg", "RSA", "-keysize", "2048", "-validity", "2", "-noprompt"])
        run([java.parent / "keytool", "-exportcert", "-keystore", key, "-storepass", "fixture-test-only",
             "-alias", "fixture", "-file", public_certificate])
        expected_certificate = sha256(public_certificate)
        generated, classes, dex = out / "generated", out / "classes", out / "dex"
        for directory in (generated, classes, dex):
            directory.mkdir(mode=0o700)
        unsigned, aligned, apk = out / "unsigned.apk", out / "aligned.apk", out / "lab-helper.apk"
        stage = "MANIFEST"
        run([tools / "aapt2", "link", "-o", unsigned, "--manifest", root / "LabManifest.xml",
             "-I", platform / "android.jar", "--java", generated])
        stage = "JAVA"
        native = root.parent.parent
        sources = [native / "src/org/tvcontrol/nativeinput/TextEngine.java"]
        sources += sorted((native / "modern/src").rglob("*.java"))
        sources += sorted((root / "src").rglob("*.java")) + sorted(generated.rglob("*.java"))
        run([java, "-m", "jdk.compiler/com.sun.tools.javac.Main", "--release", "8",
             "-Xlint:-options", "-g:none", "-encoding", "UTF-8", "-classpath", platform / "android.jar", "-d", classes, *sources])
        stage = "DEX"
        run([java, "-cp", tools / "lib/d8.jar", "com.android.tools.r8.D8", "--release", "--min-api", "31",
             "--lib", platform / "android.jar", "--output", dex, *sorted(classes.rglob("*.class"))])
        with zipfile.ZipFile(unsigned, "a") as archive:
            entry = zipfile.ZipInfo("classes.dex", date_time=(1980, 1, 1, 0, 0, 0))
            entry.external_attr = 0o100600 << 16
            archive.writestr(entry, (dex / "classes.dex").read_bytes())
        stage = "SIGN"
        run([tools / "zipalign", "-f", "4", unsigned, aligned])
        run([java, "-jar", tools / "lib/apksigner.jar", "sign", "--ks", key,
             "--ks-pass", "pass:fixture-test-only", "--out", apk, aligned])
        stage = "VERIFY_SIGNATURE"
        certificate = checked_certificate(run([java, "-jar", tools / "lib/apksigner.jar", "verify", "--print-certs", apk]))
        if certificate != expected_certificate:
            raise RuntimeError("lab APK certificate differs from its locally generated build pin")
        # Remove the disposable private key before a build receipt can be published.
        key.unlink()
        public_certificate.unlink()
        receipt = {"schema": 1, "lab_package": PACKAGE, "instrumentation_component": COMPONENT,
                   "source_commit": source_commit, "sdk_provenance_sha256": sha256(provenance),
                   "lab_apk_sha256": sha256(apk), "lab_certificate_sha256": certificate,
                   "owner_sha256": sha256(root / "client-owner.sh")}
        with (out / "lab-build-receipt.json").open("x") as output:
            json.dump(receipt, output, sort_keys=True, indent=2)
            output.write("\n")
        return receipt
    finally:
        for artifact in (key, public_certificate):
            if artifact.exists():
                artifact.unlink()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("sdk", "java", "baseline-build", "out", "provenance"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--source-commit", required=True)
    args = parser.parse_args()
    build(args.sdk, args.java, args.baseline_build, args.out, args.provenance, args.source_commit)
    print("PASS reviewed lab APK build and local signature pins")


if __name__ == "__main__":
    main()
