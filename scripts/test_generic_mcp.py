#!/usr/bin/env python3
"""Real MCP binary + bounded local fake ADB. Never contacts a TV or real account."""
import json
import os
import pathlib
import random
import select
import shlex
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import zlib

BINARY = pathlib.Path(sys.argv.pop(1) if len(sys.argv) > 1 else 'target/release/tv-control').resolve()
ROOT = pathlib.Path(__file__).resolve().parent.parent
ADB = r'''
import json,pathlib,shlex,sys
root=pathlib.Path(__file__).parent
args=sys.argv[1:]
if args==['version']:
    print('Android Debug Bridge version 1.0.41');sys.exit()
command=shlex.split(args[-1])
state_path=root/'state.json'
state=json.loads(state_path.read_text())
if command[:3]==['cmd','package','query-activities']:
    if 'android.intent.action.SEARCH' in command or 'android.intent.category.LAUNCHER' in command:
        print('No activities found')
    else:
        print('2 activities found:')
        for i,(package,label) in enumerate([('com.fixture.video','YouTube'),('com.fixture.music','YouTube Music')]):
            print(f'  Activity #{i}:\n    ActivityInfo:\n      name={package}.Main\n      packageName={package}\n      labelRes=0x0 nonLocalizedLabel={label} icon=0x0\n      ApplicationInfo:\n        packageName={package}')
elif command==['dumpsys','activity','activities']:
    print('mResumedActivity: ActivityRecord{123 u0 '+state['package']+'/.Main t1}')
elif command[:2]==['am','start']:
    component=command[command.index('-n')+1]
    state['package']=component.split('/')[0];state['launches']+=1
    state_path.write_text(json.dumps(state));print('Status: ok')
elif command==['dumpsys','media_session']:
    print('package='+state['package']+'\nstate=PlaybackState {state=3, position=1, speed=1.0}')
elif command==['screencap','-p']:
    sys.stdout.buffer.write((root/'screen.png').read_bytes())
else:
    print('unsupported fixture command',file=sys.stderr);sys.exit(2)
'''

def png(width, height, block=64):
    """Actual RGB PNG, repetitive per-block noise approximating compressed UI size."""
    rng = random.Random(4)
    raw = bytearray()
    for _ in range(height):
        raw.append(0)
        for x in range(0, width, block):
            raw.extend(bytes(rng.randrange(256) for _ in range(3)) * min(block, width-x))
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind+data))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB',width,height,8,2,0,0,0)) + chunk(b'IDAT',zlib.compress(raw,6)) + chunk(b'IEND',b'')

class GenericMcp(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.server = socket.socket()
        self.server.bind(('127.0.0.1',0)); self.server.listen(); self.server.settimeout(.1)
        self.stop = threading.Event()
        def serve():
            while not self.stop.is_set():
                try: conn,_ = self.server.accept()
                except socket.timeout: continue
                except OSError: break
                with conn:
                    conn.settimeout(2)
                    def exact(n):
                        out=b''
                        while len(out)<n:
                            part=conn.recv(n-len(out))
                            if not part: raise EOFError()
                            out+=part
                        return out
                    try:
                        request=exact(int(exact(4),16))
                        if request==b'host:version':conn.sendall(b'OKAY00040029')
                    except (OSError,EOFError,ValueError): pass
        self.thread=threading.Thread(target=serve); self.thread.start()
        self.addCleanup(self.close_server)
        self.adb=self.root/'adb'
        self.adb.write_text('#!'+sys.executable+'\n'+ADB); self.adb.chmod(0o700)
        (self.root/'state.json').write_text(json.dumps({'package':'com.fixture.home','launches':0}))
        cfg=json.loads((ROOT/'examples/config.json').read_text())
        cfg['generic_adb_control']=True
        cfg['executables']['adb']=str(self.adb)
        cfg['devices']={'screen':{'adapter':'adb','coordination_group':'screen','endpoint':'192.0.2.10:5555','server_host':'127.0.0.1','server_port':self.server.getsockname()[1],'media_app':None,'volume_max_index':None}}
        cfg['apps']={};cfg['servers']={};cfg['media']={};cfg['workflows']={}
        self.config=self.root/'config.json';self.config.write_text(json.dumps(cfg));self.config.chmod(0o600)
        self.process=None;self.serial=0;self.addCleanup(self.close_process)
        self.start()
    def close_server(self):
        self.stop.set();self.server.close();self.thread.join(timeout=3)
    def close_process(self):
        if self.process:
            p=self.process;self.process=None
            p.stdin.close()
            try:p.wait(timeout=3)
            except subprocess.TimeoutExpired:p.kill();p.wait(timeout=3)
            p.stdout.close();p.stderr.close()
    def start(self):
        self.process=subprocess.Popen([str(BINARY),'mcp',str(self.config)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        self.rpc('initialize',{'protocolVersion':'2025-11-25','capabilities':{},'clientInfo':{'name':'fixture','version':'1'}})
        self.write({'jsonrpc':'2.0','method':'notifications/initialized'})
    def write(self,value):
        self.process.stdin.write(json.dumps(value).encode()+b'\n');self.process.stdin.flush()
    def rpc(self,method,params):
        self.serial+=1;self.write({'jsonrpc':'2.0','id':self.serial,'method':method,'params':params})
        ready,_,_=select.select([self.process.stdout],[],[],12)
        self.assertTrue(ready,'MCP response timeout')
        line=self.process.stdout.readline()
        self.assertTrue(line,'MCP closed unexpectedly')
        response=json.loads(line);self.assertEqual(response.get('id'),self.serial);self.assertNotIn('error',response)
        return response['result']
    def tool(self,name,args):return self.rpc('tools/call',{'name':name,'arguments':args})
    def control(self,identity,action):
        return self.tool('tv_control',{'request_id':identity,'device':'screen','expires_at_unix_ms':int(time.time()*1000)+9000,'action':action})
    def test_name_resolution_and_restart_replay(self):
        result=self.control('open_video',{'type':'open_app','app':'YouTube'})
        self.assertEqual(result['structuredContent']['status'],'verified')
        self.assertEqual(result['structuredContent']['state']['app'],'com.fixture.video')
        self.close_process();self.start()
        result=self.control('open_video',{'type':'open_app','app':'YouTube'})
        self.assertTrue(result['structuredContent']['cached'])
        self.assertEqual(json.loads((self.root/'state.json').read_text())['launches'],1)
        result=self.control('open_video',{'type':'open_app','app':'YouTube Music'})
        self.assertEqual(result['structuredContent']['code'],'request_id_conflict')
    def test_native_search_unsupported_does_not_launch(self):
        result=self.control('search',{'type':'search','app':'YouTube','query':'cats'})
        self.assertEqual(result['structuredContent']['code'],'search_unsupported')
        self.assertEqual(json.loads((self.root/'state.json').read_text())['launches'],0)
    def test_realistic_screenshots_use_native_image_only_when_requested(self):
        for width,height in [(1920,1080),(3840,2160)]:
            image=png(width,height)
            self.assertGreater(len(image),65536);self.assertLess(len(image),1024*1024)
            (self.root/'screen.png').write_bytes(image)
            result=self.tool('tv_read',{'device':'screen','expires_at_unix_ms':int(time.time()*1000)+9000,'read':{'type':'screenshot'}})
            self.assertFalse(result['isError'],result)
            self.assertEqual(result['content'][0]['type'],'image')
            self.assertEqual(result['content'][0]['mimeType'],'image/png')
            self.assertNotIn('data',result['structuredContent'])
        result=self.control('plain',{'type':'open_app','app':'YouTube'})
        self.assertTrue(all(item['type']=='text' for item in result['content']))
    def test_oversized_screenshot_fails_without_closing_transport(self):
        image=png(3840,2160,block=16)
        self.assertGreater(len(image),1024*1024)
        (self.root/'screen.png').write_bytes(image)
        result=self.tool('tv_read',{'device':'screen','expires_at_unix_ms':int(time.time()*1000)+9000,'read':{'type':'screenshot'}})
        self.assertTrue(result['isError'])
        self.assertTrue(all(item['type']=='text' for item in result['content']))
        self.rpc('ping',{})

if __name__=='__main__':unittest.main()
