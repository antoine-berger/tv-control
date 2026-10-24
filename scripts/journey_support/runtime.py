"""Loopback-only child process fixtures and terminal/JSON-RPC clients."""
import errno
import json
import os
import pty
import re
import select
import signal
import socket
import time
import threading





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
    server.settimeout(.05)
    while not (root/'stop-adb-fixture').exists():
        try:conn,_=server.accept()
        except socket.timeout:continue
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
        while not stopping and not (root/'stop-fixture').exists() and bridge.poll() is None:
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
        (root/f'supervisor-{os.getpid()}.stopped').touch(mode=0o600)
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
    def __init__(self, args, env, binary, script=None):
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
        names=[tool['name'] for tool in response['result']['tools']]
        cursor=response['result'].get('nextCursor');seen=set();self.discovery_pages=1
        while cursor is not None:
            if cursor in seen: raise AssertionError('repeated tools cursor')
            seen.add(cursor);response=self.call('tools/list',{'cursor':cursor});self.discovery_pages+=1
            names.extend(tool['name'] for tool in response['result']['tools'])
            cursor=response['result'].get('nextCursor')
        expected = {'tv_control', 'tv_capabilities', 'tv_read', 'tv_operation', 'tv_media_read', 'tv_verify_connection'}
        if set(names) != expected: raise AssertionError(f'incomplete tool discovery: {names}')
        self.discovery_names = set(names)
        if len(names)!=len(set(names)): raise AssertionError('duplicate tools')
    def verify(self, challenge):
        response = self.call('tools/call', {'name':'tv_verify_connection','arguments':{'challenge':challenge}})
        if 'error' in response or response['result'].get('isError'): raise AssertionError(response)
        return response['result']['structuredContent']['verification_code']
    def close(self): self.socket.close()
