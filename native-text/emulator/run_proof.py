#!/usr/bin/env python3
"""Run fixture proof on an exclusively owned fresh API31 AVD; never a real device."""
import argparse
import os
from pathlib import Path
import signal
import tempfile
import subprocess
import time

MODES = ("key-remap", "normal", "hint", "password", "visible-password", "selection", "focus", "stale-window", "cancel")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", required=True, type=Path)
    parser.add_argument("--build", required=True, type=Path)
    parser.add_argument("--work", required=True, type=Path)
    args = parser.parse_args()
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("TV_NATIVE_OWNED_AVD") != "api31-disposable":
        raise SystemExit("blocked: runtime proof restricted to explicitly selected disposable CI AVD")
    args.work.mkdir(parents=True, exist_ok=False)
    adb = args.sdk / "platform-tools/adb"
    serial = "emulator-5554"
    env = dict(os.environ, ANDROID_SDK_ROOT=str(args.sdk), ANDROID_HOME=str(args.sdk),
               ANDROID_AVD_HOME=str(args.work), ANDROID_EMULATOR_HOME=str(args.work))
    def command(command, timeout=30):
        with tempfile.TemporaryFile() as output:
            process = subprocess.Popen([str(x) for x in command], env=env, stdout=output,
                                       stderr=subprocess.STDOUT, start_new_session=True)
            expires = time.monotonic() + timeout
            try:
                while process.poll() is None:
                    if os.fstat(output.fileno()).st_size > 128 * 1024:
                        raise RuntimeError("bounded fixture output exceeded")
                    if time.monotonic() >= expires:
                        raise subprocess.TimeoutExpired(str(command[0]), timeout)
                    time.sleep(0.05)
                if process.returncode:
                    raise subprocess.CalledProcessError(process.returncode, str(command[0]))
                output.seek(0)
                data = output.read(128 * 1024 + 1)
                if len(data) > 128 * 1024:
                    raise RuntimeError("bounded fixture output exceeded")
                return data.decode("utf-8", errors="strict")
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
    def shell(*words, timeout=30):
        # All words here are fixed test-owned values; never accepts user shell text.
        return command([adb, "-s", serial, "shell", *words], timeout)
    # Separate daemon startup diagnostics from the strict device-list protocol.
    command([adb, "start-server"])
    listing = command([adb, "devices"]).strip().splitlines()
    if not listing or listing[0] != "List of devices attached":
        raise SystemExit("blocked: unexpected ADB device-list response")
    devices = [line for line in listing[1:] if line.strip()]
    if devices:
        raise SystemExit("blocked: refusing any existing ADB target/session")
    avd = args.work / "proof.avd"
    avd.mkdir()
    (args.work / "proof.ini").write_text("avd.ini.encoding=UTF-8\npath=" + str(avd) + "\ntarget=android-31\n")
    (avd / "config.ini").write_text("\n".join([
        "AvdId=proof", "abi.type=x86_64", "hw.cpu.arch=x86_64", "hw.cpu.ncore=2",
        "hw.ramSize=1536", "hw.lcd.width=1080", "hw.lcd.height=1920", "hw.lcd.density=420",
        "hw.keyboard=yes", "hw.gpu.enabled=yes", "hw.gpu.mode=swiftshader_indirect",
        "disk.dataPartition.size=2G", "image.sysdir.1=system-images/android-31/default/x86_64/",
        "tag.id=default", "tag.display=Default", "PlayStore.enabled=false", ""]))
    log = (args.work / "emulator.log").open("w")
    emulator = subprocess.Popen([str(args.sdk / "emulator/emulator"), "-avd", "proof", "-port", "5554",
                                 "-no-window", "-no-audio", "-no-boot-anim", "-no-snapshot", "-wipe-data",
                                 "-gpu", "swiftshader_indirect", "-accel", "on", "-no-metrics"],
                                env=env, stdout=log, stderr=subprocess.STDOUT)
    try:
        until = time.monotonic() + 240
        while True:
            if emulator.poll() is not None:
                raise RuntimeError("owned emulator exited before boot; see local runner log")
            try:
                if shell("getprop", "sys.boot_completed", timeout=5).strip() == "1":
                    break
            except (subprocess.SubprocessError, RuntimeError):
                pass
            if time.monotonic() >= until:
                raise RuntimeError("owned emulator boot deadline exceeded")
            time.sleep(2)
        if shell("getprop", "ro.kernel.qemu").strip() != "1" or shell("getprop", "ro.build.version.sdk").strip() != "31":
            raise RuntimeError("target is not expected owned API31 emulator")
        shell("input", "keyevent", "82") # unlock owned emulator only; no audio/media controls
        command([adb, "-s", serial, "install", str(args.build / "fixture.apk")])
        command([adb, "-s", serial, "install", str(args.build / "helper.apk")])
        settings = ("default_input_method", "selected_input_method_subtype", "enabled_accessibility_services", "accessibility_enabled")
        def snapshot():
            return tuple(shell("settings", "get", "secure", key).strip() for key in settings)
        initial = snapshot()
        result = shell("am", "instrument", "-w", "org.tvcontrol.nativehelper/org.tvcontrol.nativeinput.NativeTextInstrumentation")
        if "tv_native_text_v1=refused" not in result or snapshot() != initial:
            raise RuntimeError("standalone helper entrypoint/refusal proof failed")
        command([adb, "-s", serial, "install", "-r", str(args.build / "proof.apk")])
        for mode in MODES:
            shell("am", "force-stop", "org.tvcontrol.fixture")
            shell("am", "start", "-W", "-n", "org.tvcontrol.fixture/.FixtureActivity", "--es", "mode", mode)
            time.sleep(0.5)
            if mode == "key-remap":
                shell("input", "text", "Tanya")
            result = shell("am", "instrument", "-w", "-e", "mode", mode,
                           "org.tvcontrol.nativehelper/org.tvcontrol.nativeinput.RuntimeProof", timeout=30)
            if "tv_native_runtime_proof=passed" not in result or "INSTRUMENTATION_FAILED" in result:
                raise RuntimeError("runtime fixture assertion failed: " + mode)
            if snapshot() != initial:
                raise RuntimeError("runner changed IME/accessibility secure settings: " + mode)
            print("PASS runtime fixture:", mode, "; IME/accessibility secure settings unchanged")
        print("PASS modern helper entrypoint; all proof modes passed on owned API31 only")
    finally:
        # Uninstall only our owned packages; terminate only our emulator process/target.
        for package in ("org.tvcontrol.fixture", "org.tvcontrol.nativehelper"):
            try:
                command([adb, "-s", serial, "uninstall", package], timeout=5)
            except (subprocess.SubprocessError, RuntimeError):
                pass
        # All remaining AVD data live only on the ephemeral runner.
        try:
            command([adb, "-s", serial, "emu", "kill"], timeout=5)
        except (subprocess.SubprocessError, RuntimeError):
            pass
        emulator.terminate()
        try:
            emulator.wait(timeout=10)
        except subprocess.TimeoutExpired:
            emulator.kill(); emulator.wait(timeout=5)
        log.close()


if __name__ == "__main__":
    main()
