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
import html,json,pathlib,shlex,sys
root=pathlib.Path(__file__).parent
args=sys.argv[1:]
if args==['version']:
    print('Android Debug Bridge version 1.0.41');sys.exit()
command=shlex.split(args[-1])
state_path=root/'state.json'
state=json.loads(state_path.read_text())
if command[:3]==['cmd','package','query-activities']:
    if ('android.intent.action.VIEW' in command and state.get('url_unsupported')) or 'android.intent.action.SEARCH' in command or 'android.intent.category.LAUNCHER' in command:
        print('No activities found')
    else:
        print('2 activities found:')
        for i,(package,label) in enumerate([('com.fixture.video','YouTube'),('com.fixture.music','YouTube Music')]):
            print(f'  Activity #{i}:\n    ActivityInfo:\n      name={package}.Main\n      packageName={package}\n      labelRes=0x0 nonLocalizedLabel={label} icon=0x0\n      ApplicationInfo:\n        packageName={package}')
elif command==['cmd','media_session','volume','--stream','3','--get']:
    if state.get('volume_unreadable'): print('unsupported');sys.exit()
    print('[V] volume is '+str(state.get('volume',8))+' in range [0..'+str(state.get('volume_max',15))+']')
elif command[:6]==['cmd','media_session','volume','--stream','3','--set']:
    state['volume_sets']=state.get('volume_sets',[])+[int(command[6])]
    if not state.get('fixed_volume'): state['volume']=int(command[6])
    if state.get('volume_read_lost_after'): state['volume_unreadable']=True
    state_path.write_text(json.dumps(state))
    if state.get('volume_command_fail'): sys.exit(1)
elif command==['dumpsys','power']:
    if state.get('offline'): sys.exit(1)
    dump='POWER MANAGER (dumpsys power)\n  mWakefulness='+state.get('power','Asleep')+'\n  mIsPowered=false\n'
    # Synthetic vendor diagnostic history, not just the one state line consumed.
    history='  10-24 03:00:00.000 lastWakefulness=Asleep, displayId=0, reason=fixture\n'
    size=state.get('power_dump_bytes',len(dump))
    dump+=(history*((max(0,size-len(dump))+len(history)-1)//len(history)))[:max(0,size-len(dump))]
    sys.stdout.write(dump)
elif command==['input','keyevent','224']:
    state['wakes']=state.get('wakes',0)+1
    if state.get('fail_after_wake'): state['offline']=True
    if 'power_dump_bytes_after_wake' in state: state['power_dump_bytes']=state['power_dump_bytes_after_wake']
    if state.get('power')!='VendorUnknown': state['power']='Awake'
    state_path.write_text(json.dumps(state))
elif command==['dumpsys','activity','activities']:
    print('mResumedActivity: ActivityRecord{123 u0 '+state['package']+'/.Main t1}')
elif command[:2]==['am','start']:
    component=command[command.index('-n')+1] if '-n' in command else command[command.index('-p')+1]
    state['package']=state.get('launch_foreground',component.split('/')[0]);state['launches']+=1
    state_path.write_text(json.dumps(state));print('Status: ok')
elif command==['cmd','media_session','list-sessions']:
    print('Sessions:')
    if not state.get('session_missing'):
        print('  tag=player, package='+state['package'])
        if state.get('session_duplicate'): print('  tag=player, package=com.fixture.other')
elif command==['cmd','media_session','monitor','player']:
    commands=sys.stdin.read().splitlines()
    if commands[-1:]!=['q'] or len(commands)!=2 or commands[0] not in ['play','pause','next','previous']: sys.exit(2)
    state.setdefault('media_commands',[]).append(commands[0])
    if commands[0]=='pause': state['playback']=2
    if commands[0]=='play': state['playback']=3
    state_path.write_text(json.dumps(state))
    if state.get('media_command_fail'): sys.exit(1)
    print('V2Monitoring session player... available commands: play, pause, next, previous')
elif command==['dumpsys','media_session']:
    print('  player '+state['package']+'/player/7 (userId=0)\n    ownerPid=1234, ownerUid=10001, userId=0\n    package='+state['package']+'\n    active='+str(state.get('session_active',True)).lower()+'\n    state=PlaybackState {state='+str(state.get('playback',3))+', position=12000, speed=1.0, actions='+str(state.get('media_actions',55))+'}\n    metadata: Example title, Example artist, Example album')
elif command[:2]==['sh','-c'] and 'uiautomator dump' in command[2]:
    text=html.escape(state.get('entered_text',''),quote=True)
    print('<hierarchy rotation="0"><node text="'+text+'" content-desc="Search" class="android.widget.EditText" package="'+state['package']+'" enabled="true" clickable="true" focused="true" bounds="[10,20][110,60]" /></hierarchy>')
elif command[:2]==['input','text']:
    state['text_writes']=state.get('text_writes',0)+1
    state['entered_text']=state.get('entered_text','')+command[2]
    state_path.write_text(json.dumps(state))
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
    def test_media_read_and_pause_verify_only_native_session_state(self):
        data=self.tool('tv_read',{'device':'screen','expires_at_unix_ms':int(time.time()*1000)+9000,'read':{'type':'media'}})['structuredContent']
        self.assertEqual(len(data['sessions']),1)
        session=data['sessions'][0]
        self.assertEqual(session['position_ms'],12000)
        self.assertNotIn('duration_ms',session)
        self.assertNotIn('media_id',session)
        result=self.control('media_pause',{'type':'media','session':session['session'],'command':'pause'})['structuredContent']
        self.assertEqual(result['status'],'verified',result)
        self.assertEqual(result['state']['playback'],'paused')
        self.assertNotIn('app',result['state'])
    def test_zero_native_action_mask_refuses_explicit_and_implicit_pause(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['media_actions']=0;path.write_text(json.dumps(state))
        data=self.tool('tv_read',{'device':'screen','expires_at_unix_ms':int(time.time()*1000)+9000,'read':{'type':'media'}})['structuredContent']
        self.assertEqual(data['sessions'][0]['supported_commands'],[])
        for identity,action in [('explicit_zero',{'type':'media','session':state['package']+'/player','command':'pause'}),('implicit_zero',{'type':'pause'})]:
            result=self.control(identity,action)['structuredContent']
            self.assertEqual(result['status'],'unsupported',result)
            self.assertEqual(result['code'],'media_command_unsupported')
        self.assertNotIn('media_commands',json.loads(path.read_text()))
    def test_inactive_retained_playback_is_not_verified_by_implicit_resume(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['session_active']=False;state['playback']=3;path.write_text(json.dumps(state))
        observed=self.tool('tv_read',{'device':'screen','expires_at_unix_ms':int(time.time()*1000)+9000,'read':{'type':'state'}})['structuredContent']
        self.assertNotIn('playback',observed['state'])
        result=self.tool('tv_control',{'request_id':'inactive_resume','device':'screen','expires_at_unix_ms':int(time.time()*1000)+1500,'action':{'type':'resume'}})['structuredContent']
        self.assertEqual(result['status'],'outcome_unknown',result)
        self.assertEqual(json.loads(path.read_text())['media_commands'],['play'])
    def test_unsupported_url_is_reported_as_unsupported_without_launch(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['url_unsupported']=True;path.write_text(json.dumps(state))
        result=self.control('unsupported_url',{'type':'open_url','app':'YouTube','url':'https://video.example/watch?v=1'})['structuredContent']
        self.assertEqual(result['status'],'unsupported',result)
        self.assertEqual(result['code'],'url_unsupported')
        self.assertEqual(json.loads(path.read_text())['launches'],0)
    def test_media_next_is_dispatch_only_and_replays_after_restart(self):
        path=self.root/'state.json';state=json.loads(path.read_text())
        action={'type':'media','session':state['package']+'/player','command':'next'}
        result=self.control('media_next',action)['structuredContent']
        self.assertEqual(result['status'],'observed',result)
        self.assertEqual(result['code'],'command_dispatched_state_observed')
        self.close_process();self.start()
        self.assertTrue(self.control('media_next',action)['structuredContent']['cached'])
        self.assertEqual(json.loads(path.read_text())['media_commands'],['next'])
        action['command']='previous'
        self.assertEqual(self.control('media_next',action)['structuredContent']['code'],'request_id_conflict')
    def test_media_duplicate_tag_is_rejected_without_dispatch(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['session_duplicate']=True;path.write_text(json.dumps(state))
        result=self.control('media_duplicate',{'type':'media','session':state['package']+'/player','command':'next'})['structuredContent']
        self.assertEqual(result['code'],'media_tag_ambiguous',result)
        self.assertEqual(result['status'],'failed')
        self.assertNotIn('media_commands',json.loads(path.read_text()))
    def test_media_uncertain_dispatch_never_replays(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['media_command_fail']=True;path.write_text(json.dumps(state))
        action={'type':'media','session':state['package']+'/player','command':'previous'}
        result=self.control('media_failed',action)['structuredContent']
        self.assertEqual(result['status'],'outcome_unknown',result)
        self.close_process();self.start()
        self.assertTrue(self.control('media_failed',action)['structuredContent']['cached'])
        self.assertEqual(json.loads(path.read_text())['media_commands'],['previous'])
    def test_text_entry_is_generic_and_durable_for_multiple_packages(self):
        path=self.root/'state.json'
        for package in ('com.fixture.video','org.fixture.podcasts'):
            state=json.loads(path.read_text());state['package']=package;state['entered_text']='';state['text_writes']=0;path.write_text(json.dumps(state))
            ui=self.tool('tv_read',{'device':'screen','expires_at_unix_ms':int(time.time()*1000)+9000,'read':{'type':'ui'}})['structuredContent']
            element=next(item for item in ui['elements'] if item['editable'])
            action={'type':'type_text','reference':element['ref'],'text':'cats music'}
            identity='text_'+package.replace('.','_')
            result=self.control(identity,action)['structuredContent']
            self.assertEqual(result['status'],'observed',result)
            self.assertEqual(result['state']['app'],package)
            self.close_process();self.start()
            self.assertTrue(self.control(identity,action)['structuredContent']['cached'])
            state=json.loads(path.read_text());self.assertEqual(state['text_writes'],1);self.assertEqual(state['entered_text'],'cats music')
    def test_media_mutation_invalidates_ui_reference_before_text_dispatch(self):
        ui=self.tool('tv_read',{'device':'screen','expires_at_unix_ms':int(time.time()*1000)+9000,'read':{'type':'ui'}})['structuredContent']
        ref=next(item['ref'] for item in ui['elements'] if item['editable'])
        path=self.root/'state.json';state=json.loads(path.read_text())
        result=self.control('media_before_text',{'type':'media','session':state['package']+'/player','command':'next'})['structuredContent']
        self.assertEqual(result['status'],'observed')
        result=self.control('text_after_media',{'type':'type_text','reference':ref,'text':'cats'})['structuredContent']
        self.assertEqual(result['code'],'ui_reference_unknown',result)
        self.assertNotIn('text_writes',json.loads(path.read_text()))
    def test_wake_verifies_power_and_replays_after_restart(self):
        schema=self.rpc('tools/list',{})
        control=next(t for t in schema['tools'] if t['name']=='tv_control')
        types=[kind for a in control['inputSchema']['properties']['action']['oneOf'] for kind in a['properties']['type'].get('enum',[a['properties']['type'].get('const')])]
        self.assertIn('wake',types)
        self.assertIn('mute',types)
        self.assertIn('set_volume',types)
        result=self.control('wake',{'type':'wake'})['structuredContent']
        self.assertEqual(result['status'],'verified')
        self.assertTrue(result['state']['awake'])
        self.close_process();self.start()
        result=self.control('wake',{'type':'wake'})['structuredContent']
        self.assertTrue(result['cached'])
        self.assertTrue(result['state']['awake'])
        self.control('already_awake',{'type':'wake'})
        self.assertEqual(json.loads((self.root/'state.json').read_text())['wakes'],1)
    def test_wake_offline_never_dispatches(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['offline']=True;path.write_text(json.dumps(state))
        result=self.control('offline',{'type':'wake'})['structuredContent']
        self.assertEqual(result['status'],'failed')
        self.assertEqual(result['code'],'wake_transport_unavailable')
        self.assertNotIn('wakes',json.loads(path.read_text()))
    def test_volume_discovery_mute_and_restart_deduplication(self):
        data=self.tool('tv_read',{'device':'screen','expires_at_unix_ms':int(time.time()*1000)+9000,'read':{'type':'volume'}})['structuredContent']
        self.assertEqual(data['state']['volume_index'],8)
        self.assertEqual(data['state']['volume_max_index'],15)
        caps=self.tool('tv_capabilities',{})['structuredContent']
        self.assertEqual(caps['devices'][0]['actions']['mute'],'device_dependent')
        self.assertEqual(caps['devices'][0]['actions']['set_volume'],'device_dependent')
        result=self.control('mute_once',{'type':'mute'})['structuredContent']
        self.assertEqual(result['status'],'verified',result)
        self.assertEqual(result['state']['volume_index'],0)
        self.close_process();self.start()
        self.assertTrue(self.control('mute_once',{'type':'mute'})['structuredContent']['cached'])
        self.assertEqual(self.control('mute_twice',{'type':'mute'})['structuredContent']['status'],'verified')
        self.assertEqual(json.loads((self.root/'state.json').read_text())['volume_sets'],[0])
    def test_set_volume_detects_native_scale_without_calibration(self):
        result=self.control('volume',{'type':'set_volume','index':4})['structuredContent']
        self.assertEqual(result['status'],'verified',result)
        self.assertEqual(result['state']['volume_index'],4)
        self.assertEqual(result['state']['volume_max_index'],15)
        self.close_process();self.start()
        self.assertTrue(self.control('volume',{'type':'set_volume','index':4})['structuredContent']['cached'])
        self.assertEqual(self.control('volume',{'type':'set_volume','index':0})['structuredContent']['code'],'request_id_conflict')
        result=self.control('too_high',{'type':'set_volume','index':16})['structuredContent']
        self.assertEqual(result['code'],'volume_out_of_range')
        self.assertEqual(json.loads((self.root/'state.json').read_text())['volume_sets'],[4])
    def test_volume_unavailable_never_dispatches(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['volume_unreadable']=True;path.write_text(json.dumps(state))
        for action in ({'type':'mute'},{'type':'set_volume','index':0}):
            result=self.control(action['type'],action)['structuredContent']
            self.assertEqual(result['status'],'failed',result)
            self.assertEqual(result['code'],'volume_state_unavailable')
        self.assertNotIn('volume_sets',json.loads(path.read_text()))
    def test_mute_lost_readback_never_claims_silence_or_replays(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['volume_read_lost_after']=True;path.write_text(json.dumps(state))
        result=self.control('mute_lost',{'type':'mute'})['structuredContent']
        self.assertEqual(result['status'],'outcome_unknown',result)
        self.close_process();self.start()
        self.assertTrue(self.control('mute_lost',{'type':'mute'})['structuredContent']['cached'])
        self.assertEqual(json.loads(path.read_text())['volume_sets'],[0])
    def test_mute_failed_dispatch_is_unknown_and_not_retried(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['volume_command_fail']=True;path.write_text(json.dumps(state))
        result=self.control('mute_failed',{'type':'mute'})['structuredContent']
        self.assertEqual(result['status'],'outcome_unknown',result)
        self.assertTrue(self.control('mute_failed',{'type':'mute'})['structuredContent']['cached'])
        self.assertEqual(json.loads(path.read_text())['volume_sets'],[0])
    def test_fixed_volume_does_not_verify_or_retry(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['fixed_volume']=True;path.write_text(json.dumps(state))
        result=self.tool('tv_control',{'request_id':'fixed','device':'screen','expires_at_unix_ms':int(time.time()*1000)+1500,'action':{'type':'mute'}})['structuredContent']
        self.assertEqual(result['status'],'outcome_unknown',result)
        self.assertEqual(json.loads(path.read_text())['volume_sets'],[0])
    def test_wake_large_power_dumps_ignore_text_budget(self):
        for budget in (4096,65536):
            with self.subTest(text_budget=budget):
                self.close_process()
                cfg=json.loads(self.config.read_text());cfg['preferences']['max_output_bytes']=budget;self.config.write_text(json.dumps(cfg))
                path=self.root/'state.json';state=json.loads(path.read_text());state.update(power='Asleep',wakes=0,power_dump_bytes=256*1024);path.write_text(json.dumps(state))
                self.start()
                identity='large_power_'+str(budget)
                result=self.control(identity,{'type':'wake'})['structuredContent']
                self.assertEqual(result['status'],'verified',result)
                self.assertTrue(result['state']['awake'])
                self.close_process();self.start()
                self.assertTrue(self.control(identity,{'type':'wake'})['structuredContent']['cached'])
                self.assertEqual(self.control('already_awake_'+str(budget),{'type':'wake'})['structuredContent']['status'],'verified')
                self.assertEqual(json.loads(path.read_text())['wakes'],1)
                self.rpc('ping',{})
    def test_wake_oversized_power_dump_never_dispatches(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['power_dump_bytes']=1024*1024+1;path.write_text(json.dumps(state))
        result=self.control('oversized_power',{'type':'wake'})['structuredContent']
        self.assertEqual(result['status'],'failed')
        self.assertEqual(result['code'],'adb_output_limit')
        self.assertNotIn('wakes',json.loads(path.read_text()))
        self.rpc('ping',{})
    def test_wake_oversized_postcondition_stays_unknown_without_second_key(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['power_dump_bytes_after_wake']=1024*1024+1;path.write_text(json.dumps(state))
        result=self.control('oversized_after',{'type':'wake'})['structuredContent']
        self.assertEqual(result['status'],'outcome_unknown')
        self.assertEqual(result['code'],'adb_output_limit')
        self.close_process();self.start()
        self.assertTrue(self.control('oversized_after',{'type':'wake'})['structuredContent']['cached'])
        self.assertEqual(json.loads(path.read_text())['wakes'],1)
        self.rpc('ping',{})
    def test_wake_lost_observation_stays_unknown_without_second_key(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['fail_after_wake']=True;path.write_text(json.dumps(state))
        result=self.control('lost',{'type':'wake'})['structuredContent']
        self.assertEqual(result['status'],'outcome_unknown')
        self.assertTrue(self.control('lost',{'type':'wake'})['structuredContent']['cached'])
        self.assertEqual(json.loads(path.read_text())['wakes'],1)
    def test_expired_wake_does_not_dispatch(self):
        result=self.tool('tv_control',{'request_id':'expired_wake','device':'screen','expires_at_unix_ms':1,'action':{'type':'wake'}})
        self.assertTrue(result['isError'])
        self.assertNotIn('wakes',json.loads((self.root/'state.json').read_text()))
    def test_wake_unknown_power_does_not_claim_success(self):
        path=self.root/'state.json';state=json.loads(path.read_text());state['power']='VendorUnknown';path.write_text(json.dumps(state))
        result=self.control('unknown',{'type':'wake'})['structuredContent']
        self.assertEqual(result['status'],'observed')
        self.assertEqual(result['code'],'wake_dispatched_unverified')
        self.assertNotIn('awake',result['state'])
    def test_cross_package_launch_keeps_execution_evidence_with_minimum_output_budget(self):
        self.close_process()
        cfg=json.loads(self.config.read_text());cfg['preferences']['max_output_bytes']=4096;self.config.write_text(json.dumps(cfg))
        path=self.root/'state.json';state=json.loads(path.read_text());state['launch_foreground']='org.fixture.player';path.write_text(json.dumps(state))
        self.start()
        action={'type':'open_app','app':'com.fixture.music/com.fixture.music.Main'}
        result=self.tool('tv_control',{'request_id':'cross_package','device':'screen','expires_at_unix_ms':int(time.time()*1000)+1800,'action':action})['structuredContent']
        self.assertEqual(result['status'],'outcome_unknown',result)
        self.assertEqual(result['state']['app'],'org.fixture.player')
        self.assertTrue(result['state']['connected'])
        self.assertEqual(result['state_scope'],'execution_observation')
        self.assertEqual(result['timing_scope'],'application_execution')
        self.assertLess(len(json.dumps(result)),1000)
        self.close_process();self.start()
        recovered=self.tool('tv_operation',{'request_id':'cross_package'})['structuredContent']
        self.assertEqual(recovered,result)
        replay=self.control('cross_package',action)['structuredContent']
        self.assertTrue(replay.pop('cached'))
        self.assertEqual(replay,result)
        self.assertEqual(json.loads(path.read_text())['launches'],1)
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
    def test_search_fallback_reports_dispatch_not_content(self):
        result=self.control('search',{'type':'search','app':'YouTube','query':'cats'})
        self.assertEqual(result['structuredContent']['code'],'command_dispatched_state_observed')
        self.assertEqual(result['structuredContent']['status'],'observed')
        self.assertEqual(json.loads((self.root/'state.json').read_text())['launches'],1)
    def test_open_url_dispatch_is_not_verified_playback(self):
        result=self.control('url',{'type':'open_url','app':'YouTube','url':'https://video.example/watch?v=1&list=2'})
        self.assertEqual(result['structuredContent']['status'],'observed')
        self.assertEqual(result['structuredContent']['code'],'command_dispatched_state_observed')
        self.assertEqual(json.loads((self.root/'state.json').read_text())['launches'],1)
    def test_realistic_screenshots_use_native_image_only_when_requested(self):
        for width,height,block in [(1920,1080,64),(3840,2160,64),(3840,2160,16)]:
            image=png(width,height,block)
            self.assertGreater(len(image),65536);self.assertLess(len(image),8*1024*1024)
            (self.root/'screen.png').write_bytes(image)
            result=self.tool('tv_read',{'device':'screen','expires_at_unix_ms':int(time.time()*1000)+9000,'read':{'type':'screenshot'}})
            self.assertFalse(result['isError'],result)
            self.assertEqual(result['content'][0]['type'],'image')
            self.assertEqual(result['content'][0]['mimeType'],'image/png')
            self.assertNotIn('data',result['structuredContent'])
        result=self.control('plain',{'type':'open_app','app':'YouTube'})
        self.assertTrue(all(item['type']=='text' for item in result['content']))
    def test_oversized_screenshot_fails_without_closing_transport(self):
        image=b'x' * (8*1024*1024+1)
        self.assertGreater(len(image),8*1024*1024)
        (self.root/'screen.png').write_bytes(image)
        result=self.tool('tv_read',{'device':'screen','expires_at_unix_ms':int(time.time()*1000)+9000,'read':{'type':'screenshot'}})
        self.assertTrue(result['isError'])
        self.assertEqual(result['structuredContent']['code'],'screenshot_too_large')
        self.assertTrue(all(item['type']=='text' for item in result['content']))
        self.rpc('ping',{})

if __name__=='__main__':unittest.main()
