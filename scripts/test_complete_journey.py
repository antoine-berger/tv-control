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
import termios
import time
import threading
import unittest
from unittest import mock
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

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
    address=args[args.index('-L')+1];assert address.startswith('acceptfd:')
    server=socket.socket(fileno=int(address.split(':')[1]));port=server.getsockname()[1]
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
            with (root/'adb-requests').open('ab') as log:log.write(request+b'\n')
            status=b'OKAY'
            if request==b'host:version':response=b'0029'
            elif request==b'host:devices':
                state=(root/'adb-authorization').read_bytes().strip()
                if (root/'adb-disconnected').exists():state=b'missing'
                response=b'' if state==b'missing' else b'192.0.2.10:5555\t'+state
            elif request==b'host:connect:192.0.2.10:5555':
                (root/'adb-disconnected').unlink(missing_ok=True)
                diagnostic=root/'adb-connect-diagnostic'
                response=diagnostic.read_bytes() if diagnostic.exists() else b'connected'
            elif request==b'host:disconnect:192.0.2.10:5555':
                if (root/'adb-authorization').read_text().strip()=='missing':
                    status=b'FAIL';response=b"no such device '192.0.2.10:5555'"
                else:
                    (root/'adb-disconnected').write_text('1');response=b'disconnected'
            else:raise RuntimeError('unexpected fixture request')
            conn.sendall(status+format(len(response),'04x').encode()+response)
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
    result={'alias':data['alias'],'tunnel_id':data['tunnel_id'],'profile_dir':data['profile_dir'],'profile_name':data['alias'],'stale':False,
      'process_running':running,'healthy':running,'ready':running,
      'process':{'alias':data['alias'],'tunnel_id':data['tunnel_id'],'profile_dir':data['profile_dir'],'profile_name':data['alias'],'target_kind':'command','target_value':data['command']},
      'control_plane_poll_health':{'state':'healthy'}}
    health=root/'mcp-health-url'
    if health.exists():result['mcp_health_url']=health.read_text()
    return result
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
    if (root/'mcp-health-state').exists():(root/'mcp-health-state').write_text('not_observed')
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
    if (root/'lie-about-stop').exists():
        result=status(data);result.update(process_running=False,healthy=False,ready=False)
        print(json.dumps(result));sys.exit(0)
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
        config['generic_adb_control']=True
        config['executables']['adb']=str(self.bin/'adb')
        config['devices']={'screen':{'adapter':'adb','coordination_group':'screen','endpoint':'192.0.2.10:5555','server_host':'127.0.0.1','server_port':self.port,'media_app':None,'volume_max_index':None}}
        for key in ['apps','servers','media','workflows']: config[key]={}
        self.config=self.root/'tv.json'; self.config.write_text(json.dumps(config)); self.config.chmod(0o600)
        self.connection_parent=self.config.parent
        self.env={'HOME':str(self.root),'PATH':str(self.bin)+':/usr/bin:/bin','TV_JOURNEY_CANARY':'fixture_ambient_not_for_children'}
        self.terminals=[]; self.channels=[]; self.installers=[]; self.fixture_supervisors=[]
    def tearDown(self):
        for channel in self.channels: channel.close()
        for terminal in self.terminals: terminal.close()
        state=self.runtime_state()
        owned_supervisors={process.pid:process for process in self.fixture_supervisors}
        if state:
            for key in ['supervisor_pid','bridge_pid']:
                pid=state.get(key)
                if pid in owned_supervisors and owned_supervisors[pid].poll() is not None: continue
                if pid:
                    try: os.kill(pid, signal.SIGTERM)
                    except ProcessLookupError: pass
            deadline=time.monotonic()+3
            while time.monotonic()<deadline and not self.port_free(): time.sleep(.02)
        # Restart supervisors are our direct children. Reap them before group
        # escalation: Darwin may reject killpg for an unreaped zombie-only group.
        # Once reaped, their recorded PIDs must not be signalled again.
        for supervisor in self.fixture_supervisors:
            try: supervisor.wait(timeout=5)
            except subprocess.TimeoutExpired: pass
        if state:
            for key in ['supervisor_pid','bridge_pid']:
                pid=state.get(key)
                if pid in owned_supervisors and owned_supervisors[pid].returncode is not None: continue
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
        for supervisor in self.fixture_supervisors: supervisor.wait(timeout=5)
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
    def approve(self, terminal, saved=False, restart=False):
        terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        if not saved:terminal.until('Tunnel ID:');terminal.line(TUNNEL)
        terminal.until(r'background TV controller.*\[y/N\]');terminal.line('y')
        if not saved or restart:terminal.until('Runtime key .*hidden.*:');terminal.line(SECRET)
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
    def mcp_health_fixture(self):
        root=self.root
        state=root/'mcp-health-state';state.write_text('not_observed')
        class Health(BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_GET(self):
                if self.path != '/health/mcp':self.send_error(404);return
                current=state.read_text()
                body=json.dumps({'schema_version':1,'component':'mcp',
                                 'status':'degraded' if current=='failed' else ('unknown' if current=='not_observed' else 'ok'),
                                 'state':current,'reason_code':'mcp_discovery_error' if current=='failed' else '',
                                 'details':{'child_state':'running','channel':'main','transport':'stdio'}}).encode()
                self.send_response(200);self.send_header('Content-Length',str(len(body)))
                self.end_headers();self.wfile.write(body)
        server=ThreadingHTTPServer(('127.0.0.1',0),Health)
        worker=threading.Thread(target=server.serve_forever,daemon=True);worker.start()
        (root/'mcp-health-url').write_text(f'http://127.0.0.1:{server.server_port}/health/mcp')
        def cleanup():server.shutdown();server.server_close();worker.join(timeout=2)
        self.addCleanup(cleanup)
        return state

    def test_mcp_failure_after_plugin_ui_stops_before_challenge(self):
        health=self.mcp_health_fixture()
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Tunnel ID:');terminal.line(TUNNEL)
        terminal.until(r'background TV controller.*\[y/N\]');terminal.line('y')
        terminal.until('Runtime key .*hidden.*:');terminal.line(SECRET)
        terminal.until('Press Enter when the plugin is selected')
        health.write_text('failed');terminal.line('')
        self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertIn('tunnel_mcp_discovery_failed',terminal.text)
        self.assertNotIn('Verification code returned',terminal.text)
        self.assertFalse((self.connection_parent/'connection/verification/request.json').exists())
        events=[json.loads(line)['args'] for line in (self.root/'tunnel-events.jsonl').read_text().splitlines()]
        self.assertFalse(any(args[:2]==['runtimes','stop'] for args in events))

    def test_unhealthy_owned_runtime_restart_is_explicit_and_preserves_identity(self):
        health=self.mcp_health_fixture()
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0)
        old=self.runtime_state();configuration=self.config.read_bytes()
        generation=(self.connection_parent/'connection/bridge-generation.json').read_bytes()
        health.write_text('failed')
        second=self.terminal();second.until(r'private ADB identity.*\[y/N\]');second.line('y')
        second.until('Stop this owned controller and retry');self.assertIn('tunnel_mcp_discovery_failed',second.text)
        self.assertEqual(self.runtime_state()['bridge_pid'],old['bridge_pid'])
        second.line('y');second.until(r'background TV controller.*\[y/N\]');second.line('y')
        second.until('Runtime key .*hidden.*:');second.line(SECRET)
        second.until('Press Enter when the plugin is selected');second.line('')
        challenge=second.until(r'challenge ([0-9a-f]{32})').group(1).decode()
        second.until('Verification code returned')
        current=self.runtime_state();self.assertNotEqual(current['bridge_pid'],old['bridge_pid'])
        self.assertEqual(current['alias'],old['alias']);self.assertEqual(current['tunnel_id'],old['tunnel_id'])
        self.assertEqual(self.config.read_bytes(),configuration)
        self.assertNotEqual((self.connection_parent/'connection/bridge-generation.json').read_bytes(),generation)
        second.line(self.channel().verify(challenge));self.assertEqual(second.wait(),0,second.text)
        self.assertIn('Ready.',second.text);self.assert_private_records()
        homes={event['home'] for event in map(json.loads,(self.root/'adb-events.jsonl').read_text().splitlines()) if '-L' in event['args']}
        self.assertEqual(len(homes),1)

    def test_declined_unhealthy_runtime_restart_preserves_running_controller(self):
        health=self.mcp_health_fixture()
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0)
        old=self.runtime_state();health.write_text('failed')
        before=(self.root/'adb-events.jsonl').read_bytes()
        second=self.terminal();second.until(r'private ADB identity.*\[y/N\]');second.line('y')
        second.until('Stop this owned controller and retry');second.line('n')
        self.assertNotEqual(second.wait(),0,second.text)
        self.assertEqual(self.runtime_state()['bridge_pid'],old['bridge_pid'])
        self.assertEqual((self.root/'adb-events.jsonl').read_bytes(),before)
        self.assertNotIn('Runtime key',second.text);self.assertFalse(self.port_free())
        events=[json.loads(line)['args'] for line in (self.root/'tunnel-events.jsonl').read_text().splitlines()]
        self.assertFalse(any(args[:2]==['runtimes','stop'] for args in events))

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
    def test_browser_guidance_precedes_inputs_and_permission_help_is_nonmutating(self):
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Tunnel ID:')
        self.assertIn('https://platform.openai.com/settings/organization/tunnels',terminal.text)
        self.assertIn('personal organization',terminal.text);self.assertIn('ChatGPT workspace',terminal.text)
        self.assertNotIn('Runtime key (hidden',terminal.text)
        terminal.line('help');terminal.until('Tunnel ID:')
        self.assertIn('https://platform.openai.com/settings/organization/people/roles',terminal.text)
        self.assertIn('https://platform.openai.com/settings/organization/people/groups',terminal.text)
        self.assertIn('cannot grant account access',terminal.text)
        self.assertFalse((self.root/'tunnel-events.jsonl').exists())
        terminal.line(TUNNEL);terminal.until(r'background TV controller.*\[y/N\]');terminal.line('y')
        terminal.until('Runtime key .*hidden.*:')
        self.assertIn('https://platform.openai.com/settings/organization/api-keys',terminal.text)
        self.assertIn('Restricted runtime key with Tunnels Read + Use',terminal.text)
        self.assertIn('do not use All or an admin key',terminal.text)
        self.assertIn('never into chat',terminal.text)
        terminal.line(SECRET);terminal.until('Press Enter when the plugin is selected')
        self.assertIn('https://chatgpt.com/plugins',terminal.text)
        self.assertIn('No authentication',terminal.text);self.assertIn('Do not paste the runtime key into ChatGPT',terminal.text)
        self.assertIn('type @ and select tv-control',terminal.text)
        self.assertNotIn(SECRET,terminal.text)
        terminal.line('q');self.assertNotEqual(terminal.wait(),0,terminal.text);self.assert_private_records()
    def test_cancel_at_tunnel_guide_never_requests_key_or_starts_runtime(self):
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Tunnel ID:');terminal.line('q')
        self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertNotIn('Runtime key (hidden',terminal.text)
        self.assertFalse((self.root/'tunnel-events.jsonl').exists());self.assertTrue(self.port_free())
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
    def change_configuration(self):
        configuration=json.loads(self.config.read_text())
        configuration['preferences']['max_budget_ms']-=1
        self.config.write_text(json.dumps(configuration))
    def test_changed_configuration_stops_reapproves_and_reverifies(self):
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0,first.text)
        before=self.runtime_state()['bridge_pid']
        intent=self.connection_parent/'connection/runtime-intent.json';old=intent.read_bytes()
        self.change_configuration()
        second=self.terminal();second.until('Stop the old controller and use the changed configuration');second.line('y')
        challenge=self.approve(second,saved=True,restart=True)
        self.assertNotEqual(self.runtime_state()['bridge_pid'],before)
        new=json.loads(intent.read_bytes());self.assertNotEqual(new['revision'],json.loads(old)['revision'])
        self.assertEqual(new['configuration'],self.config.read_text())
        backups=list(intent.parent.glob('runtime-intent.previous-*.json'))
        self.assertEqual(len(backups),1);self.assertEqual(backups[0].read_bytes(),old)
        second.line(self.channel().verify(challenge));self.assertEqual(second.wait(),0,second.text)
        self.assertIn('Ready.',second.text);self.assert_private_records()
    def test_rejected_configuration_change_preserves_running_binding(self):
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0,first.text)
        before=self.runtime_state()['bridge_pid']
        intent=self.connection_parent/'connection/runtime-intent.json';old=intent.read_bytes()
        self.change_configuration()
        second=self.terminal();second.until('Stop the old controller and use the changed configuration');second.line('n')
        self.assertNotEqual(second.wait(),0,second.text)
        self.assertEqual(intent.read_bytes(),old);self.assertEqual(self.runtime_state()['bridge_pid'],before)
        self.assertFalse(self.port_free());self.assertFalse(list(intent.parent.glob('runtime-intent.previous-*.json')))
        self.assertNotIn('Ready.',second.text)
    def test_changed_configuration_refuses_foreign_runtime_before_stopping(self):
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0,first.text)
        intent=self.connection_parent/'connection/runtime-intent.json';old=intent.read_bytes()
        runtime_path=intent.parent/'tunnel/fixture-runtime.json'
        runtime=json.loads(runtime_path.read_text());runtime['command']+=' foreign'
        runtime_path.write_text(json.dumps(runtime))
        self.change_configuration()
        second=self.terminal();second.until('Stop the old controller and use the changed configuration');second.line('y')
        self.assertNotEqual(second.wait(),0,second.text)
        self.assertIn('tunnel_runtime_identity_mismatch',second.text)
        self.assertEqual(intent.read_bytes(),old);self.assertFalse(self.port_free())
        self.assertFalse(list(intent.parent.glob('runtime-intent.previous-*.json')))
        events=[json.loads(line)['args'] for line in (self.root/'tunnel-events.jsonl').read_text().splitlines()]
        self.assertFalse(any(args[:2]==['runtimes','stop'] for args in events))
    def test_replacement_before_checkpoint_update_resumes_its_new_revision(self):
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0,first.text)
        state=self.connection_parent/'connection';checkpoint=state/'progress.json'
        old_checkpoint=checkpoint.read_bytes()
        self.change_configuration()
        second=self.terminal();second.until('Stop the old controller and use the changed configuration');second.line('y')
        second.until(r'private ADB identity.*\[y/N\]');second.line('q');self.assertNotEqual(second.wait(),0,second.text)
        revision=json.loads((state/'runtime-intent.json').read_text())['revision']
        # Emulate loss of the checkpoint write after atomic intent replacement.
        checkpoint.write_bytes(old_checkpoint)
        third=self.terminal();challenge=self.approve(third,saved=True,restart=True)
        self.assertNotIn('Stop the old controller',third.text)
        self.assertEqual(json.loads(checkpoint.read_text())['configuration_revision'],revision)
        third.line(self.channel().verify(challenge));self.assertEqual(third.wait(),0,third.text)
    def test_configuration_change_requires_fresh_confirmation_that_runtime_stopped(self):
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0,first.text)
        intent=self.connection_parent/'connection/runtime-intent.json';old=intent.read_bytes()
        (self.root/'lie-about-stop').write_text('fixture status must catch the still-running bridge')
        self.change_configuration()
        second=self.terminal();second.until('Stop the old controller and use the changed configuration');second.line('y')
        self.assertNotEqual(second.wait(),0,second.text)
        self.assertIn('controller_stop_unverified',second.text)
        self.assertEqual(intent.read_bytes(),old);self.assertFalse(self.port_free())
        self.assertFalse(list(intent.parent.glob('runtime-intent.previous-*.json')))
    def test_unauthorized_tv_remains_pending_and_does_not_start_tunnel(self):
        (self.root/'adb-authorization').write_text('unauthorized')
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Press Enter after TV approval');terminal.line('q')
        self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertNotIn('Ready.',terminal.text)
        self.assertIn('The TV has not authorized this private controller',terminal.text)
        self.assertFalse((self.root/'tunnel-events.jsonl').exists());self.assertTrue(self.port_free())
    def test_missing_transport_never_claims_tv_approval_and_cancel_cleans_up(self):
        (self.root/'adb-authorization').write_text('missing')
        (self.root/'adb-connect-diagnostic').write_bytes(b'failed to connect: Connection refused\x1b[31m\n')
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Press Enter to retry connection')
        self.assertNotIn('Approve this computer on the TV',terminal.text)
        self.assertNotIn('The TV has not authorized this private controller',terminal.text)
        self.assertIn('Connection refused',terminal.text)
        self.assertNotIn('\x1b',terminal.text)
        self.assertNotIn('Tunnel ID:',terminal.text)
        self.assertEqual((self.root/'adb-requests').read_text().count('host:connect:'),1)
        terminal.line('q');self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertTrue(self.port_free())
        self.assertFalse((self.root/'tunnel-events.jsonl').exists())
    def test_missing_target_disconnect_failure_still_allows_explicit_connect_retry(self):
        (self.root/'adb-authorization').write_text('missing')
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Press Enter to retry connection');terminal.line('')
        terminal.until('Press Enter to retry connection')
        requests=(self.root/'adb-requests').read_text()
        self.assertEqual(requests.count('host:connect:'),2)
        self.assertEqual(requests.count('host:disconnect:'),1)
        self.assertNotIn('Press Enter after TV approval',terminal.text)
        terminal.line('q');self.assertNotEqual(terminal.wait(),0,terminal.text);self.assertTrue(self.port_free())
    def test_offline_retry_retains_daemon_and_identity_then_waits_for_real_approval(self):
        (self.root/'adb-authorization').write_text('offline')
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Press Enter to retry connection')
        before=(self.root/'adb-events.jsonl').read_text()
        identity=self.connection_parent/'connection/adb/adb-home/identity-sentinel'
        identity.write_text('same-private-identity');identity.chmod(0o600)
        (self.root/'adb-authorization').write_text('unauthorized');terminal.line('')
        terminal.until('Press Enter after TV approval')
        requests=(self.root/'adb-requests').read_text().splitlines()
        self.assertEqual(requests.count('host:connect:192.0.2.10:5555'),2)
        self.assertEqual(requests.count('host:disconnect:192.0.2.10:5555'),1)
        self.assertFalse(any(r in ('host:kill','host:disconnect:') for r in requests))
        self.assertEqual((self.root/'adb-events.jsonl').read_text(),before)
        self.assertEqual(identity.read_text(),'same-private-identity')
        (self.root/'adb-authorization').write_text('device');terminal.line('')
        terminal.until('Tunnel ID:');terminal.line('q');self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertTrue(self.port_free())
    def test_explicit_unauthorized_retry_disconnects_only_target_without_resetting_identity(self):
        (self.root/'adb-authorization').write_text('unauthorized')
        (self.root/'adb-connect-diagnostic').write_text('failed to authenticate to 192.0.2.10:5555')
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Press Enter after TV approval')
        before=(self.root/'adb-events.jsonl').read_text()
        terminal.line('retry');terminal.until('Press Enter after TV approval')
        requests=(self.root/'adb-requests').read_text().splitlines()
        self.assertEqual(requests.count('host:disconnect:192.0.2.10:5555'),1)
        self.assertEqual(requests.count('host:connect:192.0.2.10:5555'),2)
        self.assertEqual((self.root/'adb-events.jsonl').read_text(),before)
        self.assertNotIn('Tunnel ID:',terminal.text)
        terminal.line('q');self.assertNotEqual(terminal.wait(),0,terminal.text);self.assertTrue(self.port_free())
    def test_handshake_checks_do_not_claim_approval_or_reconnect(self):
        (self.root/'adb-authorization').write_text('authorizing')
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Press Enter to check connection')
        self.assertNotIn('Press Enter after TV approval',terminal.text)
        (self.root/'adb-authorization').write_text('connecting');terminal.line('')
        terminal.until('Press Enter to check connection')
        (self.root/'adb-authorization').write_text('unauthorized');terminal.line('')
        terminal.until('Press Enter after TV approval')
        requests=(self.root/'adb-requests').read_text()
        self.assertEqual(requests.count('host:connect:'),1)
        self.assertNotIn('host:disconnect:',requests)
        terminal.line('q');self.assertNotEqual(terminal.wait(),0,terminal.text);self.assertTrue(self.port_free())
    def test_lost_unauthorized_transport_switches_to_connection_recovery(self):
        (self.root/'adb-authorization').write_text('unauthorized')
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Press Enter after TV approval')
        (self.root/'adb-authorization').write_text('missing');terminal.line('')
        terminal.until('Press Enter to retry connection')
        requests=(self.root/'adb-requests').read_text()
        self.assertEqual(requests.count('host:connect:'),1)
        self.assertNotIn('host:disconnect:',requests)
        self.assertNotIn('Tunnel ID:',terminal.text)
        terminal.line('q');self.assertNotEqual(terminal.wait(),0,terminal.text);self.assertTrue(self.port_free())
    def test_interrupt_offline_wait_keeps_no_runtime_or_tunnel(self):
        (self.root/'adb-authorization').write_text('offline')
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Press Enter to retry connection')
        os.kill(terminal.pid,signal.SIGINT)
        self.assertNotEqual(terminal.wait(),0,terminal.text);self.assertTrue(self.port_free())
        self.assertEqual((self.root/'adb-requests').read_text().count('host:connect:'),1)
        self.assertFalse((self.root/'tunnel-events.jsonl').exists())
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
    def restart_fixture_runtime(self):
        state=self.runtime_state();before=state['bridge_pid']
        generation_path=self.connection_parent/'connection/bridge-generation.json'
        old_generation=json.loads(generation_path.read_text())['generation']
        os.kill(state['supervisor_pid'],signal.SIGTERM)
        deadline=time.monotonic()+5
        while time.monotonic()<deadline and not self.port_free():time.sleep(.02)
        self.assertTrue(self.port_free())
        path=self.connection_parent/'connection/tunnel/fixture-runtime.json'
        supervisor=subprocess.Popen([sys.executable,str(self.bin/'tunnel-client'),'_supervise',str(path)],
            stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,
            env=self.env,start_new_session=True)
        self.fixture_supervisors.append(supervisor)
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            current=self.runtime_state()
            generation=json.loads(generation_path.read_text())['generation']
            if current['bridge_pid']!=before and generation!=old_generation and not self.port_free():return
            time.sleep(.02)
        self.fail('fixture bridge did not restart with fresh generation')
    def test_bridge_restart_after_code_requires_new_external_verification(self):
        terminal=self.terminal();challenge=self.approve(terminal)
        code=self.channel().verify(challenge)
        self.restart_fixture_runtime()
        terminal.line(code);self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertIn('controller_restarted_repeat_verification',terminal.text)
        self.assertNotIn('Ready.',terminal.text)
        resumed=self.terminal();challenge=self.approve(resumed,saved=True)
        resumed.line(self.channel().verify(challenge));self.assertEqual(resumed.wait(),0,resumed.text)
    def test_bridge_restart_before_tool_rejects_old_generation_challenge(self):
        terminal=self.terminal();challenge=self.approve(terminal)
        self.restart_fixture_runtime()
        response=self.channel().call('tools/call',{'name':'tv_verify_connection','arguments':{'challenge':challenge}})
        self.assertEqual(response['error']['message'],'proof_runtime_changed')
        terminal.line('q');self.assertNotEqual(terminal.wait(),0,terminal.text)
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
        saved_configuration=self.config.read_bytes()
        # A second installation shares HOME and the configuration but has never
        # owned this controller. Its uninstall must not even query the tunnel.
        other_prefix=installer.root/'another installation'
        archive=installer.asset()
        result=installer.run_script(['--prefix',str(other_prefix),'--version','v0.4.0','--archive',str(archive),'--json'],env=env)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        events=(self.root/'tunnel-events.jsonl').read_bytes()
        result=installer.run_script(['--prefix',str(other_prefix),'--uninstall','--json'],env=env)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertEqual(result.value['status'],'uninstalled')
        self.assertFalse(other_prefix.exists())
        self.assertEqual((self.root/'tunnel-events.jsonl').read_bytes(),events)
        self.assertFalse(self.port_free(), 'uninstall stopped another installation controller')
        result=installer.run_script(['--prefix',str(installer.prefix),'--uninstall','--json'],env=env)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertEqual(result.value['status'],'uninstalled')
        self.assertFalse(installer.prefix.exists())
        self.assertEqual(self.config.read_bytes(),saved_configuration)
        self.assertTrue(self.port_free(), 'uninstall left the approved controller running')
    def test_private_adb_requires_receipt_before_onboard_or_bridge_execution(self):
        self.assert_private_adb_execution_refused(False)
    def test_alias_parent_private_adb_requires_receipt_before_runtime_execution(self):
        self.assert_private_adb_execution_refused(True)
    def assert_private_adb_execution_refused(self, alias_parent):
        private=self.root/'connection-dependencies/adb-37.0.1-linux-amd64'
        private.mkdir(parents=True,mode=0o700)
        private.parent.chmod(0o700)
        binary=private/'adb';binary.write_bytes((self.bin/'adb').read_bytes());binary.chmod(0o700)
        selected=binary
        if alias_parent:
            alias=self.root/'parent-alias';alias.symlink_to(self.root,target_is_directory=True)
            selected=alias/binary.relative_to(self.root)
        config=json.loads(self.config.read_text());config['executables']['adb']=str(selected)
        self.config.write_text(json.dumps(config))
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        self.assertNotEqual(terminal.wait(),0,terminal.text)
        if sys.platform=='darwin':self.assertIn('adb_private_installation_platform_mismatch',terminal.text)
        self.assertFalse((self.root/'adb-events.jsonl').exists())
        self.assertFalse((self.root/'tunnel-events.jsonl').exists())
        state=self.root/'connection'
        checkpoint=json.loads((state/'progress.json').read_text())
        intent={'schema':1,'installation_id':checkpoint['installation_id'],
                'revision':checkpoint['configuration_revision'],'device':'screen',
                'configuration':self.config.read_text(),'controller_binary':str(BINARY)}
        intent_path=state/'runtime-intent.json';intent_path.write_text(json.dumps(intent));intent_path.chmod(0o600)
        process=subprocess.run([str(BINARY),'bridge',str(self.config),str(state)],env=self.env,
                               capture_output=True,text=True,timeout=5,start_new_session=True)
        self.assertNotEqual(process.returncode,0,process.stdout+process.stderr)
        if sys.platform=='darwin':self.assertIn('adb_private_installation_platform_mismatch',process.stdout+process.stderr)
        self.assertFalse((self.root/'adb-events.jsonl').exists())
        self.assertFalse((self.root/'tunnel-events.jsonl').exists())

    def test_configuration_without_adb_cannot_start_any_runtime(self):
        config=json.loads((ROOT/'examples/config.json').read_text())
        config['generic_adb_control']=True
        config['devices'].pop('screen');config['workflows']={}
        self.config.write_text(json.dumps(config))
        terminal=self.terminal();self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertIn('onboarding_requires_adb',terminal.text);self.assertNotIn('Ready.',terminal.text)
        self.assertFalse((self.root/'adb-events.jsonl').exists());self.assertFalse((self.root/'tunnel-events.jsonl').exists())
    def test_generic_permission_upgrade_requires_explicit_approval(self):
        config=json.loads(self.config.read_text());config.pop('generic_adb_control')
        self.config.write_text(json.dumps(config));before=self.config.read_bytes()
        terminal=self.terminal();terminal.until(r'apps installed later.*\[y/N\]');terminal.line('n')
        self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertEqual(self.config.read_bytes(),before)
        self.assertFalse((self.root/'adb-events.jsonl').exists())
        self.assertFalse((self.root/'tunnel-events.jsonl').exists())

    def test_generic_permission_upgrade_is_saved_only_after_yes(self):
        config=json.loads(self.config.read_text());config['generic_adb_control']=False
        self.config.write_text(json.dumps(config))
        terminal=self.terminal();terminal.until(r'apps installed later.*\[y/N\]');terminal.line('y')
        terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('n')
        self.assertNotEqual(terminal.wait(),0,terminal.text)
        saved=json.loads(self.config.read_text());self.assertTrue(saved.pop('generic_adb_control'))
        config.pop('generic_adb_control');self.assertEqual(saved,config)
        self.assertFalse((self.root/'adb-events.jsonl').exists())
        self.assertFalse((self.root/'tunnel-events.jsonl').exists())

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


    def assert_secret_cancellation_discards_unread_input(self, cancel_signal):
        # Keep an invoking parent alive on the same controlling terminal. Its
        # first read after the CLI exits must never inherit a partially typed key.
        parent = r"""
import os,pathlib,subprocess,sys,termios
child=subprocess.Popen(sys.argv[2:])
pathlib.Path(sys.argv[1]).write_text(str(child.pid))
assert child.wait() != 0
assert termios.tcgetattr(0)[3] & termios.ECHO
print('PARENT_READY',flush=True)
print('PARENT_READ='+repr(os.read(0,4096)),flush=True)
"""
        child_pid_file=self.root/'terminal-child.pid'
        terminal=Terminal(['-c',parent,child_pid_file,BINARY,'onboard',self.config],
                          self.env,binary=pathlib.Path(sys.executable))
        self.terminals.append(terminal)
        terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Tunnel ID:');terminal.line(TUNNEL)
        terminal.until(r'background TV controller.*\[y/N\]');terminal.line('y')
        terminal.until(r'Runtime key .*hidden.*:')
        flags=termios.tcgetattr(terminal.fd)[3]
        self.assertTrue(flags & termios.ICANON)
        self.assertFalse(flags & termios.ECHO)
        os.write(terminal.fd,SECRET.encode())  # Deliberately omit the newline.
        terminal.read(.1)
        self.assertNotIn(SECRET,terminal.text)
        os.kill(int(child_pid_file.read_text()),cancel_signal)
        terminal.until('PARENT_READY')
        terminal.line('')
        received=terminal.until(rb'PARENT_READ=([^\r\n]+)\r?\n').group(1).decode()
        self.assertEqual(terminal.wait(),0,terminal.text)
        self.assertEqual(received,repr(b'\n'))
        self.assertNotIn(SECRET,terminal.text)
        self.assertTrue(self.port_free())
        self.assertIsNone(self.runtime_state())
        self.assertFalse((self.root/'tunnel-events.jsonl').exists())
        self.assert_private_records()

    def test_sigint_discards_partial_hidden_key_before_returning_to_parent(self):
        self.assert_secret_cancellation_discards_unread_input(signal.SIGINT)

    def test_sigterm_discards_partial_hidden_key_before_returning_to_parent(self):
        self.assert_secret_cancellation_discards_unread_input(signal.SIGTERM)


class FixtureCleanup(unittest.TestCase):
    def fixture(self):
        fixture=CompleteJourney()
        fixture.channels=[];fixture.terminals=[];fixture.installers=[]
        fixture.temp=mock.Mock()
        fixture.port_free=mock.Mock(return_value=True)
        fixture.runtime_state=mock.Mock(return_value={'supervisor_pid':12345,'bridge_pid':12346})
        supervisor=mock.Mock(pid=12345,returncode=None)
        supervisor.poll.return_value=None
        fixture.fixture_supervisors=[supervisor]
        return fixture,supervisor

    def test_reaps_owned_supervisor_before_group_escalation(self):
        fixture,supervisor=self.fixture()
        def reap(timeout): supervisor.returncode=0
        supervisor.wait.side_effect=reap
        def escalate(pid,sig):
            # Simulate Darwin rejecting a zombie-only supervisor group. The
            # bridge group is inspected only after its supervisor was reaped.
            if pid==supervisor.pid or supervisor.returncode is None:
                raise PermissionError(errno.EPERM,'unreaped fixture supervisor')
        with mock.patch.object(os,'kill') as terminate, mock.patch.object(os,'killpg',side_effect=escalate) as kill_group:
            fixture.tearDown()
        terminate.assert_any_call(supervisor.pid,signal.SIGTERM)
        kill_group.assert_called_once_with(12346,signal.SIGKILL)
        supervisor.wait.assert_called_with(timeout=5)
        fixture.temp.cleanup.assert_called_once()

    def test_group_permission_error_is_not_suppressed(self):
        fixture,supervisor=self.fixture()
        def reap(timeout): supervisor.returncode=0
        supervisor.wait.side_effect=reap
        with mock.patch.object(os,'kill'), mock.patch.object(os,'killpg',side_effect=PermissionError(errno.EPERM,'fixture group denied')):
            with self.assertRaises(PermissionError): fixture.tearDown()

    def test_unresponsive_owned_supervisor_is_still_escalated_and_reaped(self):
        fixture,supervisor=self.fixture()
        supervisor.wait.side_effect=[subprocess.TimeoutExpired('fixture supervisor',5),0]
        with mock.patch.object(os,'kill'), mock.patch.object(os,'killpg') as kill_group:
            fixture.tearDown()
        kill_group.assert_any_call(supervisor.pid,signal.SIGKILL)
        self.assertEqual(supervisor.wait.call_count,2)

    def test_listener_leak_still_fails_after_supervisor_reaped(self):
        fixture,supervisor=self.fixture()
        def reap(timeout): supervisor.returncode=0
        supervisor.wait.side_effect=reap
        fixture.port_free.return_value=False
        with tempfile.TemporaryDirectory() as root, mock.patch.object(os,'kill'), mock.patch.object(os,'killpg'), mock.patch.object(time,'monotonic',side_effect=[0,4]):
            fixture.root=pathlib.Path(root)
            with self.assertRaisesRegex(AssertionError,'owned ADB listener leaked'): fixture.tearDown()
        fixture.temp.cleanup.assert_called_once()


if __name__ == '__main__': unittest.main()
