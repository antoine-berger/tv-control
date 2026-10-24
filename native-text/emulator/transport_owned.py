"""Fixed LAB launcher for an already owned AVD. Never starts or connects ADB."""
from dataclasses import dataclass
import re
import secrets
import subprocess
import time
from pathlib import Path

from owned_runtime import owns_listener, remaining
from process_group import leader_exited
from runtime_environment import LAB_COMPONENT, LAB_PACKAGE, sha256, verify_lab_build
from transport_host import Host
from transport_pipes import PipedChild
from transport_wire import TransportFault

PACKAGE, COMPONENT = LAB_PACKAGE, LAB_COMPONENT
BASE = r'/data/app/[A-Za-z0-9_/=+.~\-]+/base\.apk'



@dataclass(frozen=True)
class Pins:
    apk: Path
    owner: Path
    apk_hash: str
    owner_hash: str
    certificate_hash: str

    @classmethod
    def load(cls, build, owner, provenance, source_commit, deadline):
        """Trust the reviewed local builder receipt, never pins learned from Android.

        The builder has run apksigner verify. Equal installed APK bytes bind that
        signature too; this is local provenance, not protection from a same-UID
        attacker who can rewrite both the receipt and artifacts.
        """
        build, owner, provenance = map(Path, (build, owner, provenance))
        root = build.parent.resolve(strict=True)
        receipt = verify_lab_build(build, root, provenance, owner, source_commit, deadline)
        apk = build / 'lab-helper.apk'
        return cls(apk, owner, receipt['lab_apk_sha256'], receipt['owner_sha256'], receipt['lab_certificate_sha256'])

    def guard(self, deadline):
        if sha256(self.apk, deadline) != self.apk_hash or sha256(self.owner, deadline) != self.owner_hash:
            raise TransportFault('GUARD_FAILED')


@dataclass(frozen=True)
class ReadyAvd:
    """Created only after the runner's image/ports/API/ABI owned_ready gate."""
    adb: object
    emulator: object
    abi: str

    def guard(self, deadline):
        self.adb.guard(deadline, require_device=True)
        if leader_exited(self.emulator):
            raise TransportFault('GUARD_FAILED')
        for port in (self.adb.ports.console_port, self.adb.ports.console_port + 1):
            if not owns_listener(self.emulator.pid, port, self.adb.env, deadline=deadline):
                raise TransportFault('GUARD_FAILED')
        if leader_exited(self.emulator):
            raise TransportFault('GUARD_FAILED')
        remaining(deadline)


class OwnedLauncher:
    """No free serial/argv API; every command is fixed by this module's LAB contract."""
    def __init__(self, ready, pins):
        self.ready, self.pins = ready, pins
        self.channels = []
        self.user = self.uid = self.base = self.directory = self.owner_path = None
        self.directory_created = False

    def _read(self, *words, deadline):
        return self.ready.adb.target('shell', '-n', *words, deadline=deadline,
                                     timeout=remaining(deadline))

    def _identity(self, deadline):
        user = self._read('am', 'get-current-user', deadline=deadline)
        if not re.fullmatch(r'(0|[1-9][0-9]{0,4})\n', user):
            raise TransportFault('GUARD_FAILED')
        user = int(user)
        listing = self._read('cmd', 'package', 'list', 'packages', '-U', '--user', str(user), deadline=deadline)
        rows = []
        for line in listing.splitlines():
            match = re.fullmatch(r'package:([A-Za-z][A-Za-z0-9_.]+) uid:([0-9]+)', line)
            if match is None:
                raise TransportFault('GUARD_FAILED')
            rows.append((match[1], int(match[2])))
        candidates = [uid for package, uid in rows if package == PACKAGE]
        if (len(candidates) != 1 or len(rows) != len({package for package, _ in rows})
                or candidates[0] // 100000 != user or candidates[0] % 100000 < 10000
                or sum(uid == candidates[0] for _, uid in rows) != 1):
            raise TransportFault('GUARD_FAILED')
        path = self._read('pm', 'path', '--user', str(user), PACKAGE, deadline=deadline)
        if not path.startswith('package:') or not re.fullmatch(BASE + r'\n', path[8:]):
            raise TransportFault('GUARD_FAILED')
        base = path[8:-1]
        if '..' in base.split('/'):
            raise TransportFault('GUARD_FAILED')
        return user, candidates[0], base

    def _hash(self, path, expected, deadline):
        result = self._read('/system/bin/toybox', 'sha256sum', path, deadline=deadline)
        if result != expected + '  ' + path + '\n':
            raise TransportFault('GUARD_FAILED')

    def prepare(self, deadline):
        """Stage only the pinned owner; the client runs from the installed base APK."""
        if self.directory is not None:
            raise TransportFault('REF_CONSUMED')
        self.ready.guard(deadline)
        self.pins.guard(deadline)
        self.user, self.uid, self.base = self._identity(deadline)
        self._hash(self.base, self.pins.apk_hash, deadline)
        self.directory = '/data/local/tmp/tv-native-lab.' + secrets.token_hex(16)
        self.owner_path = self.directory + '/client-owner.sh'
        self._read('mkdir', '-m', '700', self.directory, deadline=deadline)
        self.directory_created = True
        self.ready.adb.target('push', str(self.pins.owner), self.owner_path,
                              deadline=deadline, timeout=remaining(deadline))
        self.guard(deadline)

    def guard(self, deadline):
        self.deadline = deadline
        self.ready.guard(deadline)
        self.pins.guard(deadline)
        if self.owner_path is None or self._identity(deadline) != (self.user, self.uid, self.base):
            raise TransportFault('GUARD_FAILED')
        for name, expected in (('ro.kernel.qemu', '1'), ('ro.build.version.sdk', '31'),
                               ('ro.product.cpu.abi', self.ready.abi)):
            if self._read('getprop', name, deadline=deadline) != expected + '\n':
                raise TransportFault('GUARD_FAILED')
        self._hash(self.base, self.pins.apk_hash, deadline)
        self._hash(self.owner_path, self.pins.owner_hash, deadline)
        remaining(deadline)

    def _spawn(self, words, deadline):
        self.guard(deadline)
        PipedChild.require_supported()
        adb = self.ready.adb
        remaining(deadline)
        process = subprocess.Popen([str(adb.adb), '-H', '127.0.0.1', '-P', str(adb.ports.adb_port),
                                    '-s', adb.serial, 'shell', '-T', *words], env=adb.env,
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   start_new_session=True)
        return PipedChild(process, self.channels.append)

    def instrumentation(self, instance):
        return self._spawn(['am', 'instrument', '-w', '-e', 'lab_instance', instance, COMPONENT], self.deadline)

    def client(self, instance):
        return self._spawn(['/system/bin/sh', self.owner_path, instance, self.base, str(self.uid)], self.deadline)

    def owned(self):
        return tuple(self.channels)

    def close_stage(self, deadline):
        if not self.directory_created:
            return
        self.ready.guard(deadline)
        self._read('rm', self.owner_path, deadline=deadline)
        self._read('rmdir', self.directory, deadline=deadline)
        self.directory_created = False


def connectivity(ready, pins, deadline):
    """Called only by the explicitly selected runner scenario after owned_ready."""
    launcher = OwnedLauncher(ready, pins)
    result = None
    operation_deadline = None
    try:
        launcher.prepare(deadline)
        # Staging has its own bounded budget. Host guard receives its nonrenewed
        # operation deadline; the outer runtime deadline is still an upper bound.
        operation_deadline = min(deadline, time.monotonic() + 15)
        host = Host(launcher, deadline=operation_deadline)
        result = host.probe_connectivity()
        if result.status != 'CONNECTED' or not result.qualified:
            raise TransportFault(getattr(result, 'reason', 'CLEANUP_UNVERIFIED'))
    finally:
        # Local teardown may exceed work deadline but can never retroactively
        # qualify its result; it is containment, not remote cleanup evidence.
        launcher.close_stage(time.monotonic() + 5)
    remaining(operation_deadline)
    return result
