"""Explicit runtime selection and fail-closed local SDK/build provenance checks."""
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import stat
import struct
import time
from urllib.parse import urlsplit

from process_group import require_nonreaping_wait


MAX_PROVENANCE_FILES = 32768


@dataclass(frozen=True)
class Runtime:
    mode: str
    abi: str
    cpu_arch: str
    acceleration: str
    boot_seconds: int

    @property
    def image(self):
        return f"system-images/android-31/default/{self.abi}"


def select_runtime(mode, allow_software, *, system=None, machine=None, environ=None, kvm=False):
    system = platform.system() if system is None else system
    machine = platform.machine() if machine is None else machine
    environ = os.environ if environ is None else environ
    if mode == "darwin-local":
        if system != "Darwin" or machine != "arm64" or allow_software:
            raise RuntimeError("local runtime requires native Darwin ARM64 and HVF, not software/Rosetta")
        return Runtime(mode, "arm64-v8a", "arm64", "on", 240)
    if mode != "ci-linux" or system != "Linux" or machine != "x86_64":
        raise RuntimeError("runtime mode does not match the host")
    if environ.get("GITHUB_ACTIONS") != "true" or environ.get("TV_NATIVE_OWNED_AVD") != "api31-disposable":
        raise RuntimeError("CI runtime requires explicitly selected disposable GitHub AVD")
    if not kvm and not allow_software:
        raise RuntimeError("KVM unavailable and software emulation was not explicitly selected")
    return Runtime(mode, "x86_64", "x86_64", "on" if kvm else "off", 240 if kvm else 900)


def private_directory(path):
    path = Path(path)
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise RuntimeError("local lab directory must be owned, private, and not a symlink")
    return path.resolve(strict=True)


def regular_owned(path, root):
    path = Path(path)
    if not path.resolve(strict=True).is_relative_to(root):
        raise RuntimeError("local artifact escapes the private lab")
    info = path.stat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o022:
        raise RuntimeError("local artifact is not an owned unshared regular file")
    return path


def sha256(path, deadline=None):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            if deadline is not None and time.monotonic() >= deadline:
                raise RuntimeError("local provenance verification exceeded deadline")
            digest.update(chunk)
    return digest.hexdigest()


def read_json(path):
    if path.stat().st_size > 4 * 1024 * 1024:
        raise RuntimeError("local provenance receipt exceeds bound")
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise RuntimeError("duplicate local provenance field")
            result[key] = value
        return result
    return json.loads(path.read_text(), object_pairs_hook=unique)


def properties(path):
    if path.stat().st_size > 65536:
        raise RuntimeError("SDK source properties exceed bound")
    result = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        key, separator, value = line.partition("=")
        key, value = key.strip(), value.strip()
        if not separator or key in result:
            raise RuntimeError("SDK source properties are ambiguous")
        result[key] = value
    return result


def macho_arm64(path):
    with path.open("rb") as source:
        data = source.read(4096)
    if data[:4] == b"\xcf\xfa\xed\xfe":
        return len(data) >= 8 and struct.unpack_from("<I", data, 4)[0] == 0x0100000C
    if data[:4] not in (b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf") or len(data) < 8:
        return False
    count = struct.unpack_from(">I", data, 4)[0]
    entry_size = 32 if data[:4] == b"\xca\xfe\xba\xbf" else 20
    if not 1 <= count <= 16 or 8 + count * entry_size > len(data):
        return False
    return any(struct.unpack_from(">I", data, 8 + i * entry_size)[0] == 0x0100000C
               for i in range(count))


def verify_local_inputs(sdk, build, work, provenance, runtime):
    """No executable runs here; hashes are checked against the provisioning receipt.

    The receipt records verification against official published archive metadata.
    It is trusted local configuration, not a signature/security boundary against
    another process deliberately modifying both the receipt and SDK as this user.
    """
    require_nonreaping_wait()
    sdk, build, work, provenance = map(Path, (sdk, build, work, provenance))
    if any(any(ord(char) < 32 or ord(char) == 127 for char in str(path))
           for path in (sdk, build, work, provenance)):
        raise RuntimeError("local runtime paths contain control characters")
    lab = private_directory(sdk.parent)
    info = sdk.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o022
            or not sdk.resolve(strict=True).is_relative_to(lab)):
        raise RuntimeError("SDK root must be an owned real directory in the private lab")
    if work.exists() or work.is_symlink() or not work.parent.resolve(strict=True).is_relative_to(lab):
        raise RuntimeError("runtime work path must be new and inside the private lab")
    if not build.resolve(strict=True).is_relative_to(lab):
        raise RuntimeError("runtime APK build is outside the private lab")
    regular_owned(provenance, lab)
    receipt = read_json(provenance)
    if receipt.get("schema") != 1 or receipt.get("sdk_root") != str(sdk.resolve(strict=True)):
        raise RuntimeError("SDK provenance receipt does not bind this root")
    packages = receipt.get("packages")
    if not isinstance(packages, list) or not packages or len(packages) > 32:
        raise RuntimeError("SDK provenance package set is absent or unbounded")
    destinations = set()
    for package in packages:
        source = urlsplit(package.get("archive_url", ""))
        pin = package.get("published_checksum", {})
        size = {"sha1": 40, "sha256": 64}.get(pin.get("algorithm"))
        destination = Path(package.get("sdk_path", ""))
        if (package.get("verified") is not True or source.scheme != "https"
                or source.netloc != "dl.google.com" or not source.path.startswith("/android/repository/")
                or not str(destination) or destination.is_absolute() or ".." in destination.parts
                or not isinstance(package.get("archive_size"), int) or not 0 < package["archive_size"] <= 4 * 1024 ** 3
                or size is None or not re.fullmatch(r"[0-9a-f]{" + str(size) + "}", pin.get("value", ""))
                or not re.fullmatch(r"[0-9a-f]{64}", package.get("archive_sha256", ""))):
            raise RuntimeError("SDK archive provenance is not verified official metadata")
        destinations.add(str(destination))
    if not {"platform-tools", "emulator", runtime.image} <= destinations:
        raise RuntimeError("SDK receipt does not cover the selected runtime packages")
    files = receipt.get("files")
    licenses = receipt.get("licenses", {})
    if (licenses.get("selected_google_id") != "android-sdk-license"
            or licenses.get("preview_selected") is not False or licenses.get("analytics_opted_in") is not False):
        raise RuntimeError("SDK license receipt does not cover only the selected stable packages")
    acceptance = regular_owned(sdk / "licenses/android-sdk-license", lab)
    if not 0 < acceptance.stat().st_size <= 65536 or not acceptance.read_text().strip():
        raise RuntimeError("SDK stable license acceptance is absent or empty")
    required = {"platform-tools/adb", "emulator/emulator", "licenses/android-sdk-license"}
    required.update(runtime.image + "/" + name for name in (
        "source.properties", "kernel-ranchu", "ramdisk.img", "system.img", "vendor.img", "userdata.img"))
    if not isinstance(files, dict) or not required <= files.keys() or len(files) > MAX_PROVENANCE_FILES:
        raise RuntimeError("SDK runtime file digests are missing or unbounded")
    for directory in (sdk / "platform-tools", sdk / "emulator", sdk / runtime.image):
        if any(str(path.relative_to(sdk)) not in files for path in directory.rglob("*") if path.is_file()):
            raise RuntimeError("SDK runtime tree has files outside verified provenance")
    deadline = time.monotonic() + 90
    for name, expected in files.items():
        path = Path(name)
        if path.is_absolute() or ".." in path.parts or not re.fullmatch(r"[0-9a-f]{64}", expected):
            raise RuntimeError("SDK runtime file digest has invalid syntax")
        artifact = regular_owned(sdk / path, sdk.resolve(strict=True))
        if sha256(artifact, deadline) != expected:
            raise RuntimeError("SDK runtime artifact changed since verified provisioning")
    image = properties(sdk / runtime.image / "source.properties")
    if (image.get("AndroidVersion.ApiLevel") != "31" or image.get("SystemImage.Abi") != runtime.abi
            or image.get("SystemImage.TagId") != "default"):
        raise RuntimeError("SDK image is not the exact AOSP API31 ABI selected")
    if not all(macho_arm64(sdk / binary) for binary in ("platform-tools/adb", "emulator/emulator")):
        raise RuntimeError("SDK runtime executables lack native ARM64 support")
    artifacts = read_json(regular_owned(build / "build-receipt.json", lab))
    if artifacts.get("schema") != 1:
        raise RuntimeError("APK build provenance is absent")
    for name in ("fixture.apk", "helper.apk", "proof.apk"):
        if artifacts.get("artifacts", {}).get(name) != sha256(regular_owned(build / name, lab)):
            raise RuntimeError("APK changed since the reviewed owned build")
    if shutil.disk_usage(work.parent).free < 8 * 1024 ** 3:
        raise RuntimeError("owned AVD requires at least eight GiB free")
    return receipt


def avd_config(runtime):
    return "\n".join([
        "AvdId=proof", f"abi.type={runtime.abi}", f"hw.cpu.arch={runtime.cpu_arch}", "hw.cpu.ncore=2",
        "hw.ramSize=1536", "hw.lcd.width=480", "hw.lcd.height=800", "hw.lcd.density=160",
        "hw.keyboard=yes", "hw.gpu.enabled=yes", "hw.gpu.mode=swiftshader_indirect",
        "hw.audioInput=no", "hw.audioOutput=no", "hw.camera.back=none", "hw.camera.front=none",
        "disk.dataPartition.size=2G", f"image.sysdir.1={runtime.image}/",
        "tag.id=default", "tag.display=Default", "PlayStore.enabled=false", ""])
