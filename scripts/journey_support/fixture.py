"""Owned journey environment, approval flows, and deterministic cleanup."""

import json
import os
import pathlib
import signal
import socket
import subprocess
import sys
import tempfile
import termios
import time
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from .runtime import ADB, TUNNEL_CLIENT, Terminal, Rpc

ROOT = pathlib.Path(__file__).resolve().parents[2]
TUNNEL = 'tunnel_' + 'a' * 32
SECRET = 'fixture_runtime_key_never_echo_or_save'

class JourneyFixture(unittest.TestCase):
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

    def terminal(self, binary=None, launcher=None, command='onboard'):
        binary = self.binary if binary is None else binary
        args=[command,self.config]
        if launcher: args+=['--launcher',launcher]
        terminal=Terminal(args,self.env,binary=binary);self.terminals.append(terminal);return terminal

    def approve(self, terminal, saved=False, restart=False, persist=False):
        terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        if not saved:terminal.until('Tunnel ID:');terminal.line(TUNNEL)
        terminal.until(r'background TV controller.*\[y/N\]');terminal.line('y')
        if not saved or restart:self.enter_key(terminal, persist)
        # An account UI approval is simulated here, never inferred as proof.
        terminal.until(r'Press Enter[^\n]*');terminal.line('')
        challenge=terminal.until(r'challenge ([0-9a-f]{32})').group(1).decode()
        terminal.until('Verification code returned')
        return challenge

    def enter_key(self, terminal, persist=False):
        terminal.until('Runtime key .*hidden.*:');terminal.line(SECRET)
        terminal.until('Save this runtime key');terminal.line('y' if persist else 'n')

    def prepare_migration(self, terminal):
        terminal.until('Prepare a runtime key for this migration');terminal.line('y')
        self.enter_key(terminal)

    def preserve_runtime(self, terminal):
        terminal.until('Configure a runtime key for future updates');terminal.line('n')
        self.decline_verification(terminal)
        self.assertEqual(terminal.wait(),0,terminal.text)
        self.assertIn('No new conversation verification was performed',terminal.text)
        self.assertNotIn('Verification code returned',terminal.text)
        self.assertNotIn('Runtime key (hidden',terminal.text)
        self.assertNotIn('private ADB identity',terminal.text)

    def decline_verification(self, terminal):
        terminal.until('Resume that verification now');terminal.line('n')

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

    def assert_private_records(self, allow_saved=False):
        for name in ['progress.json','events.jsonl']:
            path=self.connection_parent/'connection'/name
            if path.exists():
                self.assertEqual(path.stat().st_mode&0o777,0o600)
                self.assertNotIn(SECRET.encode(),path.read_bytes())
        for path in (self.connection_parent/'connection').rglob('*'):
            if path.is_file() and path.name not in ['session.lock','.lock']:
                if allow_saved and path.name=='runtime-credential.json':
                    self.assertEqual(path.stat().st_mode&0o777,0o600);continue
                self.assertNotIn(SECRET.encode(),path.read_bytes(),str(path))
        adb=self.root/'adb-events.jsonl'
        if adb.exists():
            for line in adb.read_text().splitlines():self.assertIsNone(json.loads(line)['ambient'])
        tunnel=self.root/'tunnel-events.jsonl'
        if tunnel.exists():
            self.assertNotIn(SECRET,tunnel.read_text())
            for line in tunnel.read_text().splitlines():self.assertFalse(json.loads(line)['admin_present'])

    def verify_existing(self):
        terminal=self.terminal(command='verify-connection')
        terminal.until('Press Enter when the plugin is selected');terminal.line('')
        challenge=terminal.until(r'challenge ([0-9a-f]{32})').group(1).decode()
        terminal.until('Verification code returned');terminal.line(self.channel().verify(challenge))
        self.assertEqual(terminal.wait(),0,terminal.text)
        for prompt in ['private ADB identity','background TV controller','Tunnel ID:', 'Runtime key (hidden']:
            self.assertNotIn(prompt,terminal.text)
        return terminal

    def assert_maintenance_no_wizard(self, terminal):
        for prompt in ['private ADB identity','background TV controller','Tunnel ID:', 'Runtime key (hidden','Verification code returned']:
            self.assertNotIn(prompt,terminal.text)
        self.assertNotIn(SECRET,terminal.text)

    def change_configuration(self):
        configuration=json.loads(self.config.read_text())
        configuration['preferences']['max_budget_ms']-=1
        self.config.write_text(json.dumps(configuration))

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
                'configuration':self.config.read_text(),'controller_binary':str(self.binary)}
        intent_path=state/'runtime-intent.json';intent_path.write_text(json.dumps(intent));intent_path.chmod(0o600)
        process=subprocess.run([str(self.binary),'bridge',str(self.config),str(state)],env=self.env,
                               capture_output=True,text=True,timeout=5,start_new_session=True)
        self.assertNotEqual(process.returncode,0,process.stdout+process.stderr)
        if sys.platform=='darwin':self.assertIn('adb_private_installation_platform_mismatch',process.stdout+process.stderr)
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
        terminal=Terminal(['-c',parent,child_pid_file,self.binary,'onboard',self.config],
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
