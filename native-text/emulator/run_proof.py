#!/usr/bin/env python3
"""Explicit CI/local proof on one fresh, privately owned API31 AVD; never a TV."""
import argparse
import json
import os
from pathlib import Path
import re
import select
import signal
import stat
import subprocess
import sys
import threading
import time

from owned_errors import summarize_owned_errors, summarize_anr_windows
from owned_proof import MODES, run_cases
from owned_runtime import (Ports, PrivateAdb, command as run_command, isolated_environment,
                           owns_listener, port_closed, sdk_processes)
from process_group import leader_exited, require_nonreaping_wait, stop_unreaped_group
from runtime_environment import avd_config, select_runtime, verify_local_inputs


def select_acceleration(kvm_usable, allow_software):
    if kvm_usable:
        return "on", 240
    if allow_software:
        return "off", 900
    raise RuntimeError("KVM unavailable and software emulation was not explicitly selected")


def drain_bounded_log(source, output, limit=64 * 1024, stop=None):
    retained = 0
    while True:
        if stop is not None:
            if stop.is_set():
                break
            fd = source.fileno()
            os.set_blocking(fd, False)
            if not select.select([fd], [], [], .1)[0]:
                continue
            try:
                chunk = os.read(fd, 4096)
            except BlockingIOError:
                continue
        else:
            chunk = source.read(4096)
        if not chunk:
            break
        keep = min(len(chunk), limit - retained)
        if keep:
            output.write(chunk[:keep])
            retained += keep
    output.flush()


def owned_diagnostics(shell):
    try:
        crash = shell("logcat", "-b", "crash", "-d", "-t", "160", "-v", "brief", "AndroidRuntime:E", "*:S", timeout=10)
        system = shell("logcat", "-b", "system", "-d", "-t", "160", "-v", "brief", "ActivityManager:E", "*:S", timeout=10)
        errors = summarize_owned_errors(crash, system)
        del crash, system
    except (OSError, subprocess.SubprocessError, RuntimeError, UnicodeError):
        errors = {"collection": "UNAVAILABLE", "crashes": [], "anrs": []}
    print("Owned error metadata:", json.dumps(errors, sort_keys=True), flush=True)
    try:
        windows = shell("dumpsys", "window", "windows", timeout=10)
        result = summarize_anr_windows(windows)
        del windows
    except (OSError, subprocess.SubprocessError, RuntimeError, UnicodeError):
        result = {"collection": "UNAVAILABLE", "owners": ["UNKNOWN"]}
    print("ANR window ownership:", json.dumps(result, sort_keys=True), flush=True)


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", required=True, choices=("ci-linux", "darwin-local"))
    parser.add_argument("--sdk", required=True, type=Path)
    parser.add_argument("--build", required=True, type=Path)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--provenance", type=Path, help="verified SDK receipt; required for darwin-local")
    parser.add_argument("--allow-software-emulation", action="store_true")
    parser.add_argument("--deadline-seconds", type=int, default=1800)
    args = parser.parse_args()
    if not 60 <= args.deadline_seconds <= 2400:
        parser.error("runtime deadline must be 60..2400 seconds")
    if args.mode == "darwin-local" and args.provenance is None:
        parser.error("darwin-local requires --provenance")
    return args


def main():
    args = arguments()
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    require_nonreaping_wait()
    try:
        kvm = stat.S_ISCHR(os.stat("/dev/kvm").st_mode) and os.access("/dev/kvm", os.R_OK | os.W_OK)
    except OSError:
        kvm = False
    runtime = select_runtime(args.mode, args.allow_software_emulation, kvm=kvm)
    if runtime.mode == "darwin-local":
        verify_local_inputs(args.sdk, args.build, args.work, args.provenance, runtime)
    args.sdk, args.build = args.sdk.resolve(strict=True), args.build.resolve(strict=True)
    os.umask(0o077)
    args.work.mkdir(mode=0o700, parents=False, exist_ok=False)
    ports = Ports()
    env = isolated_environment(args.sdk, args.work, ports.adb_port, ports.console_port)
    for name in ("home", "home/.android", "home/empty-vendor-keys", "tmp"):
        (args.work / name).mkdir(mode=0o700, exist_ok=True)
    adb = PrivateAdb(args.sdk / "platform-tools/adb", ports, env, args.work)
    emulator, log, log_thread, log_stop = None, None, None, None
    log_failed = threading.Event()
    owned_ready, completed = False, False
    phase = "preflight"
    deadline = time.monotonic() + args.deadline_seconds
    adb.deadline = deadline
    def target(*words, timeout=30):
        if log_failed.is_set():
            raise RuntimeError("owned emulator log drain failed; no new client dispatch")
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise RuntimeError("owned runtime absolute deadline exceeded; no new dispatch")
        result = adb.target(*words, timeout=min(timeout, remaining))
        if time.monotonic() >= deadline:
            raise RuntimeError("owned runtime command completed after absolute deadline")
        return result
    def shell(*words, timeout=30):
        return target("shell", "-n", *words, timeout=timeout)
    try:
        if sdk_processes(args.sdk, env):
            raise RuntimeError("private SDK already has running processes; no runtime launch")
        if runtime.mode == "darwin-local":
            if run_command(["/usr/sbin/sysctl", "-n", "kern.hv_support"], env, 5).strip() != "1":
                raise RuntimeError("HVF host support unavailable")
            memory = int(run_command(["/usr/sbin/sysctl", "-n", "hw.memsize"], env, 5).strip())
            if memory < 8 * 1024 ** 3:
                raise RuntimeError("native emulator requires at least eight GiB host RAM")
            acceleration = run_command([args.sdk / "emulator/emulator", "-accel-check"], env, 15)
            if not re.search(r"HVF|Hypervisor\.Framework", acceleration, re.I):
                raise RuntimeError("official emulator did not report HVF readiness")
        print("Owned emulator backend:", runtime.mode, runtime.abi, runtime.acceleration, flush=True)
        phase = "adb_start"
        adb.start()
        avd = args.work / "proof.avd"
        avd.mkdir(mode=0o700)
        (args.work / "proof.ini").write_text("avd.ini.encoding=UTF-8\npath=" + str(avd.resolve()) + "\ntarget=android-31\n")
        (avd / "config.ini").write_text(avd_config(runtime))
        log = (args.work / "emulator.log").open("xb")
        ports.release_console()
        phase = "emulator_boot"
        emulator = subprocess.Popen([
            str(args.sdk / "emulator/emulator"), "-avd", "proof", "-port", str(ports.console_port),
            "-no-window", "-no-audio", "-no-boot-anim", "-no-snapshot", "-wipe-data",
            "-camera-back", "none", "-camera-front", "none", "-gpu", "swiftshader_indirect",
            "-accel", runtime.acceleration, "-no-metrics"], env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
        log_stop = threading.Event()
        def drain():
            try:
                drain_bounded_log(emulator.stdout, log, 64 * 1024, log_stop)
            except BaseException:
                log_failed.set()
        log_thread = threading.Thread(target=drain, daemon=True)
        log_thread.start()
        until = min(deadline, time.monotonic() + runtime.boot_seconds)
        while True:
            if leader_exited(emulator):
                raise RuntimeError("owned emulator exited before boot; see bounded local log")
            try:
                if shell("getprop", "sys.boot_completed", timeout=5).strip() == "1":
                    break
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                pass
            if time.monotonic() >= until:
                raise RuntimeError("owned emulator boot deadline exceeded")
            time.sleep(1)
        phase = "owned_identity"
        if (not owns_listener(emulator.pid, ports.console_port, env)
                or not owns_listener(emulator.pid, ports.console_port + 1, env)):
            raise RuntimeError("emulator listeners are not owned and loopback-only")
        if (shell("getprop", "ro.kernel.qemu").strip() != "1"
                or shell("getprop", "ro.build.version.sdk").strip() != "31"
                or shell("getprop", "ro.product.cpu.abi").strip() != runtime.abi):
            raise RuntimeError("target is not the expected owned API31 ABI emulator")
        owned_ready = True
        phase = "proof"
        run_cases(args, shell, target)
        completed = True
    except (OSError, subprocess.SubprocessError, RuntimeError, UnicodeError):
        if owned_ready:
            owned_diagnostics(lambda *words, timeout=30: adb.target("shell", "-n", *words, timeout=timeout, cleanup=True))
        raise
    finally:
        primary_error = sys.exc_info()[1]
        failures = []
        if owned_ready:
            for package in ("org.tvcontrol.fixture", "org.tvcontrol.nativehelper"):
                try:
                    adb.target("uninstall", package, timeout=5, cleanup=True)
                except (OSError, subprocess.SubprocessError, RuntimeError):
                    failures.append("owned-package-cleanup")
        if emulator is not None:
            try:
                stop_unreaped_group(emulator, grace_seconds=2)
            except (OSError, RuntimeError, subprocess.SubprocessError):
                failures.append("emulator-group")
            if log_thread is not None:
                log_stop.set()
                log_thread.join(timeout=2)
                if log_thread.is_alive():
                    failures.append("emulator-log-drain")
                else:
                    emulator.stdout.close()
        if log_failed.is_set():
            failures.append("emulator-log-read")
        if log is not None and (log_thread is None or not log_thread.is_alive()):
            log.close()
        try:
            adb.close()
        except (OSError, RuntimeError, subprocess.SubprocessError):
            failures.append("adb-group-or-listener")
        ports.close()
        try:
            if sdk_processes(args.sdk, env):
                failures.append("remaining-sdk-process")
            if not all(port_closed(port) for port in (ports.adb_port, ports.console_port, ports.console_port + 1)):
                failures.append("remaining-runtime-listener")
        except (OSError, RuntimeError, subprocess.SubprocessError):
            failures.append("cleanup-observation-unavailable")
        (args.work / "runtime-receipt.json").write_text(json.dumps({
            "mode": runtime.mode, "abi": runtime.abi, "acceleration": runtime.acceleration,
            "adb_port": ports.adb_port, "console_port": ports.console_port,
            "adb_pid": adb.process.pid if adb.process else None,
            "emulator_pid": emulator.pid if emulator else None,
            "proof_completed": completed, "cleanup_verified": not failures,
            "phase": phase, "primary_error_type": type(primary_error).__name__ if primary_error else None,
            "adb_startup_diagnostic": getattr(adb, "startup_diagnostic", None),
            "cleanup_failures": failures}, indent=2) + "\n")
        if failures:
            raise RuntimeError("owned runtime cleanup unverified: " + ",".join(failures)) from primary_error
        print("PASS owned runtime cleanup; no SDK process/listener remains", flush=True)


if __name__ == "__main__":
    main()
