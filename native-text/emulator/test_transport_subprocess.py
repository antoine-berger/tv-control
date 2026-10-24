"""Real owned Python children stand in for adb/am/client; no Android executable."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from transport_host import Host
from transport_pipes import PipedChild
from test_transport_bundle import PUBLIC_INSTANCE

PEER = r'''
import os, select, struct, sys, time
role, mode, instance, fd = sys.argv[1:]
fd = int(fd)
def frame(op, fields=b''):
    body=struct.pack('>IBB',0x4e544c31,1,op)+fields
    return struct.pack('>I',len(body))+body
def line(key,value,prefix='INSTRUMENTATION_RESULT: '):
    sys.stdout.buffer.write((prefix+key+'='+value+'\n').encode())
def exact(size):
    value=b''
    while len(value)<size:
        chunk=os.read(0,size-len(value))
        if not chunk: return None
        value+=chunk
    return value
def command():
    head=exact(4)
    if head is None: return None
    size,=struct.unpack('>I',head)
    if size>16380: return None
    return exact(size)
if role=='am':
    for key,value in [('tv_native_lab_protocol_v1','1'),('tv_native_lab_instance_v1',instance),('tv_native_lab_ready_v1','READY')]:
        line(key,value,'INSTRUMENTATION_STATUS: ')
    sys.stdout.buffer.write(b'INSTRUMENTATION_STATUS_CODE: 1\n');sys.stdout.buffer.flush()
    os.read(fd,1)
    if mode=='no_bundle': sys.exit(0)
    if mode=='oversized':
        sys.stdout.buffer.write(b'x'*9000);sys.stdout.buffer.flush();sys.exit(0)
    count=2 if mode=='double_bundle' else 1
    for _ in range(count):
        for key,value in [('tv_native_lab_protocol_v1','1'),('tv_native_lab_instance_v1',instance),('tv_native_text_v1','verified'),('tv_native_cleanup_v1','VERIFIED'),('tv_native_entered_v1','YES'),('tv_native_reason_v1','NONE')]:
            line(key,value)
        sys.stdout.buffer.write(b'INSTRUMENTATION_CODE: 0\n')
    sys.stdout.buffer.flush()
else:
    if command() is None: sys.exit(0)
    ref=frame(0x81,b'e'*16+b'r'*16+struct.pack('>q',4000))
    if mode=='slow':
        for value in ref:
            sys.stdout.buffer.write(bytes([value]));sys.stdout.buffer.flush();time.sleep(.02)
    else:
        sys.stdout.buffer.write(ref);sys.stdout.buffer.flush()
    if mode=='premature_eof': sys.exit(0)
    request=command()
    if request is None: sys.exit(0)
    if request[5]!=2: sys.exit(2)
    if mode=='stderr':
        sys.stderr.buffer.write(request[42:]);sys.stderr.buffer.flush()
    os.write(fd,b'D')
    sys.stdout.buffer.write(frame(0x82,b'\x01'));sys.stdout.buffer.flush()
    if mode=='survivor':
        while os.read(0,1): pass
        sys.exit(0)
    if mode=='no_remote_owner': sys.exit(0)
    # EOF after a one-shot communicate(input) would arrive here before the
    # witness. It is not accepted as a successful host half-close.
    if select.select([0],[],[],.05)[0] and os.read(0,1)==b'': sys.exit(3)
    sys.stdout.buffer.write(frame(0x83,bytes.fromhex(instance)+b'\x00'))
    sys.stdout.buffer.flush()
'''


class Launcher:
    def __init__(self, directory, mode):
        self.directory, self.mode = directory, mode
        self.script = directory / 'fake_peer.py'
        self.script.write_text(PEER)
        self.channels = []
        self.argv = []
        self.environments = []
        self.read_fd, self.write_fd = os.pipe()
        self.guards = 0

    def guard(self):
        self.guards += 1  # Fake owned-runtime guard; no ADB implementation is selected.

    def _start(self, role, instance, descriptor):
        argv = [sys.executable, '-B', '-u', str(self.script), role, self.mode, instance, str(descriptor)]
        environment = {'PATH': os.defpath, 'LANG': 'C', 'LC_ALL': 'C'}
        self.argv.append(argv)
        self.environments.append(environment)
        PipedChild.require_supported()  # Before any real owned child exists.
        process = subprocess.Popen(argv, env=environment, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   pass_fds=(descriptor,), start_new_session=True)
        child = PipedChild(process, self.channels.append)
        return child

    def instrumentation(self, instance):
        return self._start('am', instance, self.read_fd)

    def client(self, instance):
        return self._start('client', instance, self.write_fd)

    def owned(self):
        return tuple(self.channels)

    def close_control(self):
        os.close(self.read_fd)
        os.close(self.write_fd)


@unittest.skipUnless(all(hasattr(os, key) for key in ('waitid', 'P_PID', 'WNOWAIT')), 'requires non-reaping process ownership')
class RealPipes(unittest.TestCase):
    def run_case(self, mode, budget=2):
        with tempfile.TemporaryDirectory(prefix='lab-host-fake-') as directory:
            launcher = Launcher(Path(directory), mode)
            try:
                result = Host(launcher, entropy=lambda: PUBLIC_INSTANCE, budget=budget).run('PRIVATE_STDIN_CANARY')
                self.assertTrue(all(child.process.returncode is not None for child in launcher.channels))
                self.assertTrue(all(all(getattr(child.process, name).closed for name in ('stdin', 'stdout', 'stderr'))
                                    for child in launcher.channels))
                combined = str(launcher.argv) + str(launcher.environments) + str(result.public())
                for private in ['PRIVATE_STDIN_CANARY', 'eeeeeeee', 'rrrrrrrr']:
                    self.assertNotIn(private, combined)
                return result
            finally:
                launcher.close_control()

    def test_private_stdin_positive_two_channel_completion(self):
        result = self.run_case('valid')
        self.assertEqual(result.status, 'VERIFIED')
        self.assertTrue(result.qualified)

    def test_local_zero_without_remote_bundle_or_owner_is_unknown(self):
        for mode in ('no_bundle', 'no_remote_owner'):
            with self.subTest(mode=mode):
                result = self.run_case(mode)
                self.assertEqual(result.status, 'UNKNOWN')
                self.assertEqual(result.public()['cleanup'], 'UNVERIFIED')

    def test_eof_before_commit_does_not_mean_refusal_or_remote_cleanup(self):
        result = self.run_case('premature_eof', budget=.4)
        self.assertEqual(result.status, 'UNKNOWN')
        self.assertFalse(result.qualified)
        self.assertEqual(result.client_cleanup, 'UNVERIFIED')

    def test_client_surviving_verified_server_bundle_does_not_qualify(self):
        result = self.run_case('survivor', budget=.4)
        self.assertEqual(result.status, 'UNKNOWN')
        self.assertEqual(result.client_cleanup, 'UNVERIFIED')
        self.assertFalse(result.qualified)

    def test_duplicate_oversized_or_private_stderr_never_leaks_or_verifies(self):
        for mode in ('double_bundle', 'oversized', 'stderr'):
            with self.subTest(mode=mode):
                result = self.run_case(mode)
                self.assertEqual(result.status, 'UNKNOWN')
                self.assertFalse(result.qualified)

    def test_pipe_setup_exception_still_has_registered_child_cleanup(self):
        with mock.patch('transport_pipes.os.set_blocking', side_effect=OSError('PRIVATE_STDIN_CANARY')):
            result = self.run_case('valid')
        self.assertEqual(result.status, 'UNKNOWN')
        self.assertFalse(result.qualified)

    def test_slow_partial_output_does_not_reset_deadline(self):
        result = self.run_case('slow', budget=.2)
        self.assertEqual(result.status, 'UNKNOWN')
        self.assertEqual(result.reason, 'HOST_DEADLINE')


if __name__ == '__main__':
    unittest.main()
