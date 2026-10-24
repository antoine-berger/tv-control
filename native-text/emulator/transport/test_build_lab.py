#!/usr/bin/env python3
"""Builder contract with all executable/tool calls mocked; never invokes SDK or Java."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

import build_lab


PIN = "a" * 40


class BuilderTest(unittest.TestCase):
    def test_manifest_has_only_the_self_targeted_lab_instrumentation(self):
        root = ET.parse(Path(__file__).with_name("LabManifest.xml")).getroot()
        android = "{http://schemas.android.com/apk/res/android}"
        self.assertEqual(root.attrib["package"], build_lab.PACKAGE)
        self.assertEqual([child.tag for child in root], ["uses-sdk", "application", "instrumentation"])
        self.assertEqual(root.find("uses-sdk").attrib[android + "minSdkVersion"], "31")
        application = root.find("application")
        self.assertEqual(list(application), [])
        self.assertEqual(application.attrib[android + "allowBackup"], "false")
        self.assertNotIn(android + "debuggable", application.attrib)
        instrumentation = root.find("instrumentation")
        self.assertEqual(instrumentation.attrib[android + "targetPackage"], build_lab.PACKAGE)
        self.assertEqual(instrumentation.attrib[android + "name"], "org.tvcontrol.nativeinput.LabSocketInstrumentation")

    def fixture(self, root):
        sdk, java_home = root / "sdk", root / "java"
        java_files = {"bin/java": b"java", "bin/keytool": b"key", "lib/modules": b"modules"}
        required = {"platforms/android-31/android.jar", "build-tools/35.0.0/aapt2", "build-tools/35.0.0/zipalign",
                    "build-tools/35.0.0/lib/d8.jar", "build-tools/35.0.0/lib/apksigner.jar"}
        for name, content in java_files.items():
            file = java_home / name
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(content)
        for name in required:
            file = sdk / name
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(b"sdk")
        receipt = {"java_home": str(java_home), "java": {"verified": True,
                   "files": {name: hashlib.sha256(value).hexdigest() for name, value in java_files.items()}},
                   "files": {name: hashlib.sha256(b"sdk").hexdigest() for name in required}}
        provenance = root / "provision.json"
        provenance.write_text("{}")
        return sdk, java_home / "bin/java", receipt, provenance

    def test_preflight_java_and_build_tools_are_pinned(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sdk, java, receipt, provenance = self.fixture(root)
            with patch.object(build_lab, "verify_local_inputs", return_value=receipt) as verified, \
                    patch.object(build_lab, "select_runtime", return_value="reviewed"), \
                    patch.object(build_lab, "macho_arm64", return_value=True):
                self.assertEqual(build_lab.preflight(sdk, java, root / "baseline", root / "new", provenance), java.resolve())
                verified.assert_called_once_with(sdk, root / "baseline", root / "new", provenance, "reviewed")
                java.write_bytes(b"corrupted")
                with self.assertRaisesRegex(RuntimeError, "changed"):
                    build_lab.preflight(sdk, java, root / "baseline", root / "new", provenance)
                java.write_bytes(b"java")
                (java.parent / "unrecorded").write_bytes(b"extra")
                with self.assertRaisesRegex(RuntimeError, "additional"):
                    build_lab.preflight(sdk, java, root / "baseline", root / "new", provenance)
                (java.parent / "unrecorded").unlink()
                receipt["files"].pop("platforms/android-31/android.jar")
                with self.assertRaisesRegex(RuntimeError, "requires verified API31"):
                    build_lab.preflight(sdk, java, root / "baseline", root / "new", provenance)

    def test_signature_parser_rejects_missing_duplicate_and_multiple_signers(self):
        digest = "b" * 64
        line = "Signer #1 certificate SHA-256 digest: " + digest
        self.assertEqual(build_lab.checked_certificate(line), digest)
        for invalid in ("", line + "\n" + line, line + "\nSigner #2 certificate SHA-256 digest: " + digest,
                        "Signer #1 certificate SHA-256 digest: invalid"):
            with self.assertRaises(RuntimeError):
                build_lab.checked_certificate(invalid)

    def command_fake(self, root, records, *, fail=None, dirty=False, wrong_certificate=False):
        certificate = hashlib.sha256(b"public certificate").hexdigest()
        def fake(argv, env, timeout=30):
            args = [str(value) for value in argv]
            records.append((args, dict(env)))
            if args[0] == "/usr/bin/git":
                return "dirty" if "status" in args and dirty else "" if "status" in args else PIN
            if fail and fail in args:
                raise RuntimeError("unlogged private build-tool details")
            def after(option):
                return Path(args[args.index(option) + 1])
            if "-genkeypair" in args:
                after("-keystore").write_bytes(b"private test key")
            elif "-exportcert" in args:
                after("-file").write_bytes(b"public certificate")
            elif "link" in args:
                import zipfile
                with zipfile.ZipFile(after("-o"), "w"):
                    pass
            elif "jdk.compiler/com.sun.tools.javac.Main" in args:
                (after("-d") / "Fixture.class").write_bytes(b"mock class")
            elif "com.android.tools.r8.D8" in args:
                (after("--output") / "classes.dex").write_bytes(b"mock dex")
            elif Path(args[0]).name == "zipalign":
                shutil.copyfile(args[-2], args[-1])
            elif "sign" in args:
                shutil.copyfile(args[-1], after("--out"))
            elif "verify" in args:
                return "Signer #1 certificate SHA-256 digest: " + ("0" * 64 if wrong_certificate else certificate)
            else:
                raise AssertionError("unexpected executable path")
            return ""
        return fake

    def test_fake_build_pins_signature_and_deletes_private_key_before_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sdk, java, _, provenance = self.fixture(root)
            output = root / "out"
            records = []
            with patch.object(build_lab, "preflight", return_value=java), \
                    patch.object(build_lab, "command", side_effect=self.command_fake(root, records)):
                result = build_lab.build(sdk, java, root / "baseline", output, provenance, PIN)
            self.assertEqual(result, json.loads((output / "lab-build-receipt.json").read_text()))
            self.assertEqual(result["lab_apk_sha256"], build_lab.sha256(output / "lab-helper.apk"))
            self.assertEqual(result["lab_certificate_sha256"], hashlib.sha256(b"public certificate").hexdigest())
            self.assertEqual(result["lab_package"], "org.tvcontrol.nativehelper")
            self.assertEqual(result["source_commit"], PIN)
            self.assertFalse((output / "ephemeral-test-key.p12").exists())
            self.assertFalse((output / "ephemeral-certificate.der").exists())
            self.assertEqual(output.stat().st_mode & 0o777, 0o700)
            self.assertEqual((output / "lab-build-receipt.json").stat().st_mode & 0o777, 0o600)
            for argv, env in records:
                self.assertFalse(any("adb" == Path(arg).name or "emulator" == Path(arg).name for arg in argv))
                self.assertLessEqual(set(env), {"PATH", "LANG", "LC_ALL", "HOME", "TMPDIR", "JAVA_HOME"})
            compiler = [argv for argv, _ in records if "jdk.compiler/com.sun.tools.javac.Main" in argv]
            self.assertEqual(len(compiler), 1)
            self.assertEqual(compiler[0][compiler[0].index("--release") + 1], "8")
            self.assertEqual(compiler[0][compiler[0].index("-classpath") + 1], str(sdk / "platforms/android-31/android.jar"))
            self.assertTrue(all(flag not in compiler[0] for flag in ("-bootclasspath", "-source", "-target", "--system")))
            dex = [argv for argv, _ in records if "com.android.tools.r8.D8" in argv]
            self.assertEqual(len(dex), 1)
            self.assertEqual(dex[0][dex[0].index("--min-api") + 1], "31")
            self.assertEqual(dex[0][dex[0].index("--lib") + 1], str(sdk / "platforms/android-31/android.jar"))

    def test_dirty_or_wrong_source_stops_before_output_or_sdk(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sdk, java, _, provenance = self.fixture(root)
            records = []
            with patch.object(build_lab, "preflight", return_value=java), \
                    patch.object(build_lab, "command", side_effect=self.command_fake(root, records, dirty=True)):
                with self.assertRaisesRegex(RuntimeError, "clean"):
                    build_lab.build(sdk, java, root / "baseline", root / "out", provenance, PIN)
            self.assertFalse((root / "out").exists())
            self.assertTrue(all(argv[0] == "/usr/bin/git" for argv, _ in records))

    def test_tool_failure_and_certificate_mismatch_never_publish_receipt(self):
        for failure in ("-genkeypair", "-exportcert", "link", "jdk.compiler/com.sun.tools.javac.Main",
                        "com.android.tools.r8.D8", "sign", "verify", "WRONG_CERTIFICATE"):
            with self.subTest(stage=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                sdk, java, _, provenance = self.fixture(root)
                output, records = root / "out", []
                with patch.object(build_lab, "preflight", return_value=java), \
                        patch.object(build_lab, "command", side_effect=self.command_fake(
                            root, records, fail=failure, wrong_certificate=failure == "WRONG_CERTIFICATE")):
                    with self.assertRaises(RuntimeError) as error:
                        build_lab.build(sdk, java, root / "baseline", output, provenance, PIN)
                self.assertNotIn("private build-tool", str(error.exception))
                self.assertFalse((output / "lab-build-receipt.json").exists())
                self.assertFalse((output / "ephemeral-test-key.p12").exists())


if __name__ == "__main__":
    unittest.main()
