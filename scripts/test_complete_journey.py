#!/usr/bin/env python3
"""Real terminal + Rust MCP journey with private, loopback-only process fixtures.

The test supplies simulated ADB authorization, tunnel supervision and a cooperative
user copying a code. It does not establish access to a TV, account or hosted chat.
"""
import errno
import json
import os
import pathlib
import pty
import re
import select
import shlex
import signal
import socket
import shutil
import subprocess
import sys
import tempfile
import time
import threading
import unittest

BINARY = pathlib.Path(sys.argv.pop(1)).resolve() if len(sys.argv) > 1 else pathlib.Path('target/debug/tv-control').resolve()
ROOT = pathlib.Path(__file__).resolve().parent.parent
TUNNEL = 'tunnel_' + 'a' * 32
SECRET = 'fixture_runtime_key_never_echo_or_save'

ADB = r'''
import json,os,pathlib,shlex,socket,sys
root=pathlib.Path(__file__).parent.parent
args=sys.argv[1:]
with (root/'adb-events.jsonl').open('a') as f:f.write(json.dumps({'args':args,'pid':os.getpid(),'home':os.environ.get('HOME'),'ambient':os.environ.get('TV_JOURNEY_CANARY')})+'\n')
if args==['version']:print('Android Debug Bridge version 1.0.41');sys.exit(0)
if '-L' in args:
    port=int(args[args.index('-L')+1].split(':')[-1]);server=socket.socket();server.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1);server.bind(('127.0.0.1',port));server.listen()
    def exact(conn,n):
        out=b''
        while len(out)<n:
            piece=conn.recv(n-len(out))
            if not piece:raise EOFError()
            out+=piece
        return out
    while True:
        conn,_=server.accept()
        with conn:
            try:request=exact(conn,int(exact(conn,4),16))
            except (EOFError,ValueError):continue
            if request==b'host:version':response=b'0029'
            elif request==b'host:devices':response=b'192.0.2.10:5555\t'+(root/'adb-authorization').read_bytes().strip()
            elif request==b'host:connect:192.0.2.10:5555':response=b'connected'
            else:raise RuntimeError('unexpected fixture request')
            conn.sendall(b'OKAY'+format(len(response),'04x').encode()+response)
            try:conn.recv(1)
            except ConnectionResetError:pass
elif args[-1:] == ['get-state']: print((root/'adb-authorization').read_text().strip())
elif len(args)>1 and args[-2]=='shell' and shlex.split(args[-1])==['dumpsys','activity','activities']:
    print('mResumedActivity: ActivityRecord{0 org.example.player/.Main t1}')
else:sys.exit(91)
'''

TUNNEL_CLIENT = r'''
import json,os,pathlib,select,shlex,signal,socket,subprocess,sys,time
root=pathlib.Path(__file__).parent.parent
def load(path):return json.loads(path.read_text())
def write(path,value):
    temporary=path.with_name(path.name+'.tmp-'+str(os.getpid()))
    temporary.write_text(json.dumps(value));temporary.chmod(0o600);temporary.replace(path)
def alive(pid):
    try:os.kill(pid,0);return True
    except ProcessLookupError:return False
def status(data):
    running=alive(data['supervisor_pid']) and alive(data['bridge_pid'])
    return {'alias':data['alias'],'tunnel_id':data['tunnel_id'],'profile_dir':data['profile_dir'],'profile_name':data['alias'],'stale':False,
      'process_running':running,'healthy':running,'ready':running,
      'process':{'alias':data['alias'],'tunnel_id':data['tunnel_id'],'profile_dir':data['profile_dir'],'profile_name':data['alias'],'target_kind':'command','target_value':data['command']},
      'control_plane_poll_health':{'state':'healthy'}}
if sys.argv[1:2]==['_supervise']:
    path=pathlib.Path(sys.argv[2]);data=load(path);stopping=False
    def stop(*_):
        global stopping
        stopping=True
    signal.signal(signal.SIGTERM,stop);signal.signal(signal.SIGINT,stop)
    bridge=subprocess.Popen(shlex.split(data['command']),stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=os.environ.copy(),start_new_session=True)
    server=socket.socket();server.bind(('127.0.0.1',0));server.listen();server.setblocking(False)
    data.update(supervisor_pid=os.getpid(),bridge_pid=bridge.pid,proxy_port=server.getsockname()[1]);write(path,data)
    conn=None
    try:
        while not stopping and bridge.poll() is None:
            readers=[server,bridge.stdout,bridge.stderr]+([conn] if conn else [])
            ready,_,_=select.select(readers,[],[],0.05)
            for source in ready:
                if source is server:
                    fresh,_=server.accept()
                    if conn:conn.close()
                    conn=fresh
                elif source is bridge.stderr:
                    message=os.read(bridge.stderr.fileno(),4096)
                    # Bounded fixture diagnostic only, never record credential env.
                    if message:
                        old=(root/'bridge-stderr').read_bytes() if (root/'bridge-stderr').exists() else b''
                        (root/'bridge-stderr').write_bytes((old+message)[-16384:])
                elif source is bridge.stdout:
                    message=os.read(bridge.stdout.fileno(),65536)
                    if message and conn:conn.sendall(message)
                elif source is conn:
                    message=conn.recv(65536)
                    if message:bridge.stdin.write(message);bridge.stdin.flush()
                    else:conn.close();conn=None
    finally:
        if conn:conn.close()
        server.close()
        if bridge.poll() is None:
            os.killpg(bridge.pid,signal.SIGTERM)
            try:bridge.wait(timeout=3)
            except subprocess.TimeoutExpired:os.killpg(bridge.pid,signal.SIGKILL);bridge.wait(timeout=2)
        for pipe in [bridge.stdin,bridge.stdout,bridge.stderr]:pipe.close()
    sys.exit(0)
args=sys.argv[1:];state=pathlib.Path(os.environ['TUNNEL_CLIENT_STATE_DIR']);path=state/'fixture-runtime.json'
with (root/'tunnel-events.jsonl').open('a') as f:f.write(json.dumps({'args':args,'pid':os.getpid(),'admin_present':bool(os.environ.get('OPENAI_ADMIN_KEY'))})+'\n')
assert args[:1]==['runtimes']
if args[1]=='list':
    data=load(path) if path.exists() else None
    print(json.dumps({'aliases':[{'alias':data['alias'],'tunnel_id':data['tunnel_id'],'profile_dir':data['profile_dir'],'profile_name':data['alias']}] if data else []}))
elif args[1]=='connect':
    def flag(name):return args[args.index(name)+1]
    assert flag('--runtime-api-key')=='env:TV_CONTROL_TUNNEL_KEY'
    assert os.environ.get('TV_CONTROL_TUNNEL_KEY')
    data={'alias':flag('--alias'),'tunnel_id':flag('--tunnel-id'),'profile_dir':flag('--profile-dir'),'command':flag('--mcp-command')}
    write(path,data)
    daemon=subprocess.Popen([sys.executable,__file__,'_supervise',str(path)],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
    deadline=time.monotonic()+5
    while time.monotonic()<deadline:
        data=load(path)
        if data.get('bridge_pid'):break
        if daemon.poll() is not None:raise RuntimeError('fixture supervisor stopped')
        time.sleep(.01)
    assert data.get('bridge_pid');print(json.dumps(status(data)))
elif args[1]=='status':print(json.dumps(status(load(path))))
elif args[1]=='stop':
    data=load(path)
    if alive(data['supervisor_pid']):os.kill(data['supervisor_pid'],signal.SIGTERM)
    deadline=time.monotonic()+5
    while time.monotonic()<deadline and alive(data['bridge_pid']):time.sleep(.02)
    result=status(data);result.update(process_running=False,healthy=False,ready=False);print(json.dumps(result))
else:raise RuntimeError('unexpected fixture command')
'''


class Terminal:
    def __init__(self, args, env, binary=BINARY, script=None):
        input_pipe = os.pipe() if script is not None else None
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            try:
                if input_pipe:
                    os.close(input_pipe[1]); os.dup2(input_pipe[0], 0); os.close(input_pipe[0])
                os.execve(str(binary), [str(binary), *map(str, args)], env)
            except BaseException: os._exit(90)
        self.output = bytearray(); self.cursor = 0; self.status = None
        self.feeder = None; self.feed_errors = []
        if input_pipe:
            os.close(input_pipe[0])
            def feed():
                try:
                    for offset in range(0, len(script), 97): os.write(input_pipe[1], script[offset:offset+97])
                except BrokenPipeError: pass
                except BaseException as exc: self.feed_errors.append(exc)
                finally: os.close(input_pipe[1])
            self.feeder = threading.Thread(target=feed, daemon=True); self.feeder.start()
    def read(self, duration=.1):
        ready, _, _ = select.select([self.fd], [], [], duration)
        if ready:
            try:
                data = os.read(self.fd, 65536)
                self.output.extend(data)
                return bool(data)
            except OSError as exc:
                if exc.errno != errno.EIO: raise
        return False
    def until(self, pattern, timeout=12):
        regex = re.compile(pattern.encode() if isinstance(pattern, str) else pattern)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            match = regex.search(self.output, self.cursor)
            if match:
                self.cursor = match.end(); return match
            self.read()
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.status = os.waitstatus_to_exitcode(status)
                self.read(0)
                match = regex.search(self.output, self.cursor)
                if match: self.cursor = match.end(); return match
                break
        raise AssertionError(f'missing terminal pattern {pattern!r}\n{self.text}')
    @property
    def text(self): return self.output.decode(errors='replace')
    def line(self, text): os.write(self.fd, text.encode() + b'\n')
    def wait(self, timeout=10):
        deadline = time.monotonic() + timeout
        while self.status is None and time.monotonic() < deadline:
            self.read(.05)
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid: self.status = os.waitstatus_to_exitcode(status)
        if self.status is None: raise AssertionError('terminal process did not exit\n' + self.text)
        for _ in range(3): self.read(0)
        return self.status
    def close(self):
        if self.status is None:
            try: os.killpg(self.pid, signal.SIGTERM)
            except ProcessLookupError: pass
            try: self.wait(3)
            except AssertionError:
                os.killpg(self.pid, signal.SIGKILL); os.waitpid(self.pid, 0)
        os.close(self.fd)
        if self.feeder:
            self.feeder.join(timeout=2)
            if self.feeder.is_alive() or self.feed_errors: raise AssertionError('installer input feeder did not close cleanly')


class Rpc:
    def __init__(self, port):
        self.socket = socket.create_connection(('127.0.0.1', port), timeout=5)
        self.pending = b''; self.next_id = 1
    def send(self, value): self.socket.sendall(json.dumps(value).encode() + b'\n')
    def call(self, method, params):
        request_id = self.next_id; self.next_id += 1
        self.send({'jsonrpc':'2.0','id':request_id,'method':method,'params':params})
        while True:
            while b'\n' not in self.pending:
                data = self.socket.recv(65536)
                if not data: raise AssertionError('fixture bridge closed without a response')
                self.pending += data
            line, self.pending = self.pending.split(b'\n', 1)
            response = json.loads(line)
            if response.get('id') == request_id: return response
    def initialize(self):
        response = self.call('initialize', {'protocolVersion':'2025-11-25','capabilities':{},'clientInfo':{'name':'isolated-journey-contract','version':'1'}})
        if 'error' in response: raise AssertionError(response)
        self.send({'jsonrpc':'2.0','method':'notifications/initialized'})
        response = self.call('tools/list', {})
        if 'tv_verify_connection' not in [tool['name'] for tool in response['result']['tools']]: raise AssertionError(response)
    def verify(self, challenge):
        response = self.call('tools/call', {'name':'tv_verify_connection','arguments':{'challenge':challenge}})
        if 'error' in response or response['result'].get('isError'): raise AssertionError(response)
        return response['result']['structuredContent']['verification_code']
    def close(self): self.socket.close()


class CompleteJourney(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="tv journey's ")
        self.root = pathlib.Path(self.temp.name).resolve(); self.root.chmod(0o700)
        self.bin = self.root/'bin'; self.bin.mkdir(mode=0o700)
        for name, body in [('adb', ADB), ('tunnel-client', TUNNEL_CLIENT)]:
            path=self.bin/name; path.write_text('#!'+sys.executable+'\n'+body); path.chmod(0o700)
        (self.root/'adb-authorization').write_text('device')
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0)); self.port=listener.getsockname()[1]
        config=json.loads((ROOT/'examples/config.json').read_text())
        config['executables']['adb']=str(self.bin/'adb')
        config['devices']={'screen':{'adapter':'adb','coordination_group':'screen','endpoint':'192.0.2.10:5555','server_host':'127.0.0.1','server_port':self.port,'media_app':None,'volume_max_index':None}}
        for key in ['apps','servers','media','workflows']: config[key]={}
        self.config=self.root/'tv.json'; self.config.write_text(json.dumps(config)); self.config.chmod(0o600)
        self.connection_parent=self.config.parent
        self.env={'HOME':str(self.root),'PATH':str(self.bin)+':/usr/bin:/bin','TV_JOURNEY_CANARY':'fixture_ambient_not_for_children'}
        self.terminals=[]; self.channels=[]; self.installers=[]
    def tearDown(self):
        for channel in self.channels: channel.close()
        for terminal in self.terminals: terminal.close()
        state=self.runtime_state()
        if state:
            for key in ['supervisor_pid','bridge_pid']:
                pid=state.get(key)
                if pid:
                    try: os.kill(pid, signal.SIGTERM)
                    except ProcessLookupError: pass
            deadline=time.monotonic()+3
            while time.monotonic()<deadline and not self.port_free(): time.sleep(.02)
            for key in ['supervisor_pid','bridge_pid']:
                pid=state.get(key)
                if pid:
                    try: os.killpg(pid, signal.SIGKILL)
                    except ProcessLookupError: pass
        free=self.port_free()
        # Preserve the failure while cleaning up only daemons recorded by this
        # fixture. A failing/forcibly killed terminal must not leak its fake ADB.
        if not free:
            events=self.root/'adb-events.jsonl'
            if events.exists():
                for line in events.read_text().splitlines():
                    event=json.loads(line)
                    if '-L' in event['args']:
                        try:os.killpg(event['pid'],signal.SIGKILL)
                        except ProcessLookupError:pass
        for installer in self.installers: installer.tearDown()
        self.temp.cleanup()
        self.assertTrue(free, 'owned ADB listener leaked after fixture teardown')
    def runtime_state(self):
        path=self.connection_parent/'connection/tunnel/fixture-runtime.json'
        try:return json.loads(path.read_text())
        except (FileNotFoundError,json.JSONDecodeError):return None
    def port_free(self):
        with socket.socket() as listener:
            listener.settimeout(.1)
            return listener.connect_ex(('127.0.0.1',self.port)) != 0
    def terminal(self, binary=BINARY, launcher=None):
        args=['onboard',self.config]
        if launcher: args+=['--launcher',launcher]
        terminal=Terminal(args,self.env,binary=binary);self.terminals.append(terminal);return terminal
    def approve(self, terminal, saved=False):
        terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        if not saved:terminal.until('Tunnel ID:');terminal.line(TUNNEL)
        terminal.until(r'background TV controller.*\[y/N\]');terminal.line('y')
        if not saved:terminal.until('Runtime key .*hidden.*:');terminal.line(SECRET)
        # An account UI approval is simulated here, never inferred as proof.
        terminal.until(r'Press Enter[^\n]*');terminal.line('')
        challenge=terminal.until(r'challenge ([0-9a-f]{32})').group(1).decode()
        terminal.until('Verification code returned')
        return challenge
    def channel(self):
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            state=self.runtime_state()
            if state and state.get('proxy_port'):break
            time.sleep(.02)
        else:raise AssertionError('fixture runtime proxy missing')
        channel=Rpc(state['proxy_port']);self.channels.append(channel);channel.initialize();return channel
    def assert_private_records(self):
        for name in ['progress.json','events.jsonl']:
            path=self.connection_parent/'connection'/name
            if path.exists():
                self.assertEqual(path.stat().st_mode&0o777,0o600)
                self.assertNotIn(SECRET.encode(),path.read_bytes())
        for path in (self.connection_parent/'connection').rglob('*'):
            if path.is_file() and path.name not in ['session.lock','.lock']:
                self.assertNotIn(SECRET.encode(),path.read_bytes(),str(path))
        adb=self.root/'adb-events.jsonl'
        if adb.exists():
            for line in adb.read_text().splitlines():self.assertIsNone(json.loads(line)['ambient'])
        tunnel=self.root/'tunnel-events.jsonl'
        if tunnel.exists():
            self.assertNotIn(SECRET,tunnel.read_text())
            for line in tunnel.read_text().splitlines():self.assertFalse(json.loads(line)['admin_present'])
    def test_complete_cooperative_fixture_requires_real_readonly_mcp_call(self):
        terminal=self.terminal();challenge=self.approve(terminal)
        self.assertNotIn('Ready.',terminal.text)
        code=self.channel().verify(challenge);self.assertRegex(code,r'^[0-9a-f]{32}$')
        terminal.line(code);self.assertEqual(terminal.wait(),0,terminal.text)
        self.assertIn('Ready.',terminal.text);self.assertNotIn(SECRET,terminal.text)
        self.assert_private_records()
    def test_wrong_code_never_reports_ready(self):
        terminal=self.terminal();challenge=self.approve(terminal)
        self.channel().verify(challenge)
        terminal.line('0'*32);self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertNotIn('Ready.',terminal.text);self.assert_private_records()
    def test_pending_conversation_resumes_with_same_command(self):
        first=self.terminal();self.approve(first);first.line('q')
        self.assertNotEqual(first.wait(),0,first.text);self.assertNotIn('Ready.',first.text)
        before=self.runtime_state()['bridge_pid']
        second=self.terminal();challenge=self.approve(second,saved=True)
        self.assertEqual(self.runtime_state()['bridge_pid'],before)
        second.line(self.channel().verify(challenge));self.assertEqual(second.wait(),0,second.text)
        self.assertIn('Ready.',second.text);self.assertNotIn('Runtime key',second.text);self.assert_private_records()
    def test_typed_code_without_an_mcp_call_is_not_evidence(self):
        terminal=self.terminal();self.approve(terminal)
        terminal.line('0'*32);self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertNotIn('Ready.',terminal.text);self.assert_private_records()
    def test_unauthorized_tv_remains_pending_and_does_not_start_tunnel(self):
        (self.root/'adb-authorization').write_text('unauthorized')
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Press Enter after TV approval');terminal.line('q')
        self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertNotIn('Ready.',terminal.text)
        self.assertIn('Approve this computer on the TV',terminal.text)
        self.assertFalse((self.root/'tunnel-events.jsonl').exists());self.assertTrue(self.port_free())
    def test_tv_approval_keeps_owned_identity_alive_until_confirmed(self):
        (self.root/'adb-authorization').write_text('unauthorized')
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Press Enter after TV approval')
        self.assertFalse(self.port_free(), 'the pending authorization runtime was stopped too early')
        before=[json.loads(line)['pid'] for line in (self.root/'adb-events.jsonl').read_text().splitlines() if '-L' in json.loads(line)['args']]
        (self.root/'adb-authorization').write_text('device');terminal.line('')
        terminal.until(r'\[2/4\]');terminal.until('Tunnel ID:');terminal.line('q')
        self.assertNotEqual(terminal.wait(),0,terminal.text)
        after=[json.loads(line)['pid'] for line in (self.root/'adb-events.jsonl').read_text().splitlines() if '-L' in json.loads(line)['args']]
        self.assertEqual(before,after);self.assertTrue(self.port_free());self.assertNotIn('Ready.',terminal.text)
    def test_bridge_loss_after_proof_cannot_produce_final_ready(self):
        terminal=self.terminal();challenge=self.approve(terminal)
        code=self.channel().verify(challenge)
        os.kill(self.runtime_state()['bridge_pid'],signal.SIGTERM)
        deadline=time.monotonic()+3
        while time.monotonic()<deadline and not self.port_free():time.sleep(.02)
        self.assertTrue(self.port_free())
        terminal.line(code);self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertNotIn('Ready.',terminal.text)
    def test_stable_launcher_resumes_across_binary_paths_and_disconnects(self):
        first_binary=self.root/'binary-a'; second_binary=self.root/'binary-b'
        for binary in [first_binary,second_binary]:shutil.copyfile(BINARY,binary);binary.chmod(0o700)
        pointer=self.root/'selected-binary';pointer.write_text(str(first_binary))
        launcher=self.root/'controller';launcher.write_text('#!/bin/sh\nexecutable=$(cat '+shlex.quote(str(pointer))+')\nexec "$executable" "$@"\n');launcher.chmod(0o700)
        first=self.terminal(binary=first_binary,launcher=launcher);self.approve(first);first.line('q')
        self.assertNotEqual(first.wait(),0,first.text);bridge_pid=self.runtime_state()['bridge_pid']
        pointer.write_text(str(second_binary))
        second=self.terminal(binary=second_binary,launcher=launcher);challenge=self.approve(second,saved=True)
        self.assertEqual(self.runtime_state()['bridge_pid'],bridge_pid)
        second.line(self.channel().verify(challenge));self.assertEqual(second.wait(),0,second.text)
        process=subprocess.run([str(second_binary),'disconnect',str(self.config),'--json'],env=self.env,capture_output=True,text=True,timeout=8,start_new_session=True)
        self.assertEqual(process.returncode,0,process.stdout+process.stderr)
        self.assertFalse(json.loads(process.stdout)['runtime_running']);self.assertTrue(self.port_free())
    def test_zero_argument_piped_installer_reaches_verified_native_journey(self):
        # Reuse asset/download fixtures, but the installed executable is the real
        # binary under test. An existing private config avoids any LAN discovery.
        import test_installer as installer_fixture
        installer=installer_fixture.Installer();installer.setUp();self.installers.append(installer)
        installer.native_stub=BINARY.read_bytes()
        env,downloads,_=installer.zero_argument_environment()
        env['PATH']=str(self.bin)+os.pathsep+str(installer.bin)+':/usr/bin:/bin'
        env['TV_JOURNEY_CANARY']=self.env['TV_JOURNEY_CANARY']
        configuration=pathlib.Path(env['XDG_CONFIG_HOME'])/'tv-control/tv-control.local.json'
        configuration.parent.mkdir(parents=True,mode=0o700)
        configuration.write_bytes(self.config.read_bytes());configuration.chmod(0o600)
        self.config=configuration;self.connection_parent=configuration.parent
        terminal=Terminal(['-s'],env,binary=pathlib.Path('/bin/sh'),script=(ROOT/'install.sh').read_bytes())
        self.terminals.append(terminal)
        terminal.until('Continue\\? Type yes:');terminal.line('yes')
        challenge=self.approve(terminal)
        terminal.line(self.channel().verify(challenge));self.assertEqual(terminal.wait(15),0,terminal.text)
        self.assertIn('Ready.',terminal.text);self.assertNotIn('Choose the display',terminal.text)
        self.assertEqual(len(downloads.read_text().splitlines()),2)
        self.assertTrue((installer.prefix/'tv-control').is_file());self.assert_private_records()
    def test_configuration_without_adb_cannot_start_any_runtime(self):
        config=json.loads((ROOT/'examples/config.json').read_text())
        config['devices'].pop('screen');config['workflows']={}
        self.config.write_text(json.dumps(config))
        terminal=self.terminal();self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertIn('onboarding_requires_adb',terminal.text);self.assertNotIn('Ready.',terminal.text)
        self.assertFalse((self.root/'adb-events.jsonl').exists());self.assertFalse((self.root/'tunnel-events.jsonl').exists())
    def test_adb_refusal_spawns_no_device_or_tunnel_child(self):
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('n')
        self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertFalse((self.root/'adb-events.jsonl').exists())
        self.assertFalse((self.root/'tunnel-events.jsonl').exists())
        self.assertFalse((self.root/'connection/adb').exists())
        self.assertNotIn('Ready.',terminal.text)
    def test_interrupt_releases_owned_adb_without_starting_tunnel(self):
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Tunnel ID:');os.kill(terminal.pid,signal.SIGINT)
        self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertTrue(self.port_free())
        self.assertFalse((self.root/'tunnel-events.jsonl').exists())
        self.assertNotIn('Ready.',terminal.text)
    def test_agent_json_and_no_terminal_cannot_grant_access(self):
        for arguments in [['onboard',str(self.config),'--json'],['onboard',str(self.config)]]:
            process=subprocess.run([str(BINARY),*arguments],env=self.env,stdin=subprocess.DEVNULL,capture_output=True,text=True,timeout=5,start_new_session=True)
            self.assertNotEqual(process.returncode,0)
            if '--json' in arguments:
                self.assertEqual(len(process.stdout.splitlines()),1)
                self.assertFalse(json.loads(process.stdout)['ready'])
            self.assertFalse((self.root/'adb-events.jsonl').exists())
            self.assertFalse((self.root/'tunnel-events.jsonl').exists())


if __name__ == '__main__': unittest.main()
