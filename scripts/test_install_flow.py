#!/usr/bin/env python3
"""Zero-argument piped installer + real Rust wizard; private HOME and wire fixtures only.

The native fixture launcher adds --fixture to the installed process's arguments.
Production code has no environment switch that can bypass discovery or authorization.
"""
import json,os,pathlib,subprocess,sys,tempfile,unittest
from test_installer import Installer

BINARY=pathlib.Path(sys.argv.pop(1)).resolve()
LEGACY=pathlib.Path(sys.argv.pop(1)).resolve() if len(sys.argv)>1 else None
ROOT=pathlib.Path(__file__).resolve().parent.parent

class InstallFlow(unittest.TestCase):
 @classmethod
 def setUpClass(cls):
  cls.compiler=tempfile.TemporaryDirectory(prefix='tv-flow-launcher-');root=pathlib.Path(cls.compiler.name)
  source=root/'launcher.c';source.write_text(r'''#include <stdlib.h>
#include <unistd.h>
#include <string.h>
int main(int argc,char **argv) {
 const char *binary=getenv("TV_FLOW_BINARY"),*fixture=getenv("TV_FLOW_FIXTURE");
 if(!binary||!fixture)return 90;
 char **next=calloc((size_t)argc+5,sizeof(char*));if(!next)return 91;
 for(int i=0;i<argc;i++)next[i]=argv[i];next[0]=(char*)binary;
 if(argc>1&&!strcmp(argv[1],"setup")){next[argc]="--fixture";next[argc+1]=(char*)fixture;}
 if(argc>1&&!strcmp(argv[1],"onboard")){argc=3;next[3]=0;const char *flow=getenv("TV_FLOW_ONBOARD_FIXTURE");if(!flow)return 93;next[argc]="--fixture";next[argc+1]=(char*)flow;next[argc+2]="--json";}
 execv(binary,next);return 92;
}
''');binary=root/'launcher';subprocess.run(['cc',str(source),'-o',str(binary)],check=True,capture_output=True);cls.launcher=binary.read_bytes()
 @classmethod
 def tearDownClass(cls):cls.compiler.cleanup()
 def setUp(self):
  self.h=Installer();self.h.setUp();self.h.native_stub=self.launcher
  self.env,self.downloads,_=self.h.zero_argument_environment();self.env['TV_FLOW_BINARY']=str(BINARY)
  self.data=json.loads((ROOT/'examples/discovery.json').read_text());self.fixture=self.h.root/'network.json';self.write_fixture()
  self.env['TV_FLOW_FIXTURE']=str(self.fixture)
  journey=self.h.root/'journey.json';journey.write_text('{"schema":1}');self.env['TV_FLOW_ONBOARD_FIXTURE']=str(journey)
  adb=self.h.bin/'adb';adb.write_text('#!/bin/sh\nexit 77\n');adb.chmod(0o700)
  self.output=pathlib.Path(self.env['XDG_CONFIG_HOME'])/'tv-control/tv-control.local.json';self.checkpoint=self.output.with_name(self.output.name+'.setup-state.json')
 def tearDown(self):self.h.tearDown()
 def write_fixture(self):self.fixture.write_text(json.dumps(self.data))
 def run_install(self,responses):
  return self.h.run_terminal([],responses,env=self.env,fragmented=True)
 def first_failure(self):
  self.data['failure']={'stage':'mdns_send','os_error':13};self.write_fixture()
  p=self.run_install([('Continue? Type yes:',b'yes\n'),('Type manual, or Enter to stop:',b'\n')])
  self.assertEqual(p.returncode,2,p.stdout);self.assertIn('OS error: 13',p.stdout);self.assertIn('Searching for TVs',p.stdout);self.assertNotIn('query this LAN',p.stdout);self.assertTrue((self.h.prefix/'tv-control').exists());self.assertTrue(self.checkpoint.exists());self.assertFalse(self.output.exists())
 def assert_saved(self):
  self.assertTrue(self.output.exists());self.assertEqual(self.output.stat().st_mode&0o777,0o600);self.assertTrue(json.loads(self.checkpoint.read_text())['configuration_saved'])
 def test_install_failure_identical_rerun_and_completed_configuration(self):
  self.first_failure();self.data.pop('failure');self.write_fixture()
  p=self.run_install([('Continue? Type yes:',b'yes\n'),('Choose the display:',b'1\n'),('Local name [',b'\n')]);self.assertEqual(p.returncode,0,p.stdout);self.assertIn('Resuming local setup',p.stdout);self.assert_saved();original=self.output.read_bytes();self.assertEqual(len(self.downloads.read_text().splitlines()),2)
  self.data['failure']={'stage':'mdns_send','os_error':13};self.write_fixture();p=self.run_install([('Continue? Type yes:',b'yes\n')]);self.assertEqual(p.returncode,0,p.stdout);self.assertNotIn('Find TVs on',p.stdout);self.assertEqual(self.output.read_bytes(),original);self.assertEqual(len(self.downloads.read_text().splitlines()),2)
 def test_upgrade_from_v022_preserves_existing_configuration(self):
  original_payload=self.h.native_stub
  if LEGACY is not None:self.h.native_stub=LEGACY.read_bytes()
  old=self.h.asset('v0.2.2');self.h.native_stub=original_payload;p=self.h.install(old,'v0.2.2');self.assertEqual(p.returncode,0,p.stdout+p.stderr)
  self.output.parent.mkdir(mode=0o700,parents=True)
  p=subprocess.run([str(LEGACY or BINARY),'setup','--fixture',str(self.fixture),'--interface','lan-fixture','--device','screen_192_0_2_10','--adb',str(self.h.bin/'adb'),'--curl',str(self.h.bin/'curl'),'--output',str(self.output),'--json','--confirm'],capture_output=True,text=True,timeout=5);self.assertEqual(p.returncode,0,p.stdout+p.stderr);before=self.output.read_bytes()
  self.data['failure']={'stage':'mdns_send','os_error':13};self.write_fixture();p=self.run_install([('Continue? Type yes:',b'yes\n')]);self.assertEqual(p.returncode,0,p.stdout);self.assertEqual(self.output.read_bytes(),before);self.assertEqual((self.h.prefix/'current').read_text(),'v0.3.1\n');self.assertTrue((self.h.prefix/'versions/v0.2.2').exists());self.assertNotIn('Find TVs on',p.stdout)
 def test_corrupt_progress_is_backed_up_then_setup_continues(self):
  self.first_failure();self.checkpoint.write_text('{interrupted');self.data.pop('failure');self.write_fixture()
  p=self.run_install([('Continue? Type yes:',b'yes\n'),('Keep a private backup',b'yes\n'),('Choose the display:',b'1\n'),('Local name [',b'\n')]);self.assertEqual(p.returncode,0,p.stdout);self.assert_saved();backups=list(self.output.parent.glob('*.corrupt-*'));self.assertEqual(len(backups),1);self.assertEqual(backups[0].read_text(),'{interrupted')
 def test_disappeared_saved_display_offers_new_selection(self):
  p=self.run_install([('Continue? Type yes:',b'yes\n'),('Choose the display:',b'1\n'),('Local name [',b'q\n')]);self.assertEqual(p.returncode,1,p.stdout);self.assertFalse(self.output.exists())
  for packet in self.data['packets']:
   if packet.get('peer')=='192.0.2.10':packet['peer']='192.0.2.40';packet['hex']=packet['hex'].replace('c000020a','c0000228')
  self.write_fixture();p=self.run_install([('Continue? Type yes:',b'yes\n'),('Choose the display:',b'3\n'),('Local name [',b'\n')]);self.assertEqual(p.returncode,0,p.stdout);self.assertIn('saved display is no longer advertised',p.stdout);self.assertEqual(json.loads(self.output.read_text())['devices']['screen']['endpoint'],'192.0.2.40:5555');self.assertEqual(json.loads(self.checkpoint.read_text())['device'],'screen_192_0_2_40')
 def test_empty_discovery_manual_endpoint_survives_interruption(self):
  self.data['packets']=[];self.write_fixture()
  p=self.run_install([('Continue? Type yes:',b'yes\n'),("Enter the TV's connection IPv4:port",b'192.0.2.10:5555\n'),('Connection protocol:',b'legacy\n'),('Local name [',b'q\n')]);self.assertEqual(p.returncode,1,p.stdout);state=json.loads(self.checkpoint.read_text());self.assertEqual(state['manual_endpoint'],'192.0.2.10:5555');self.assertEqual(state['manual_kind'],'legacy')
  p=self.run_install([('Continue? Type yes:',b'yes\n'),('Is the saved TV connection endpoint',b'yes\n'),('Local name [',b'\n')]);self.assertEqual(p.returncode,0,p.stdout);self.assertNotIn('Find TVs on',p.stdout);self.assert_saved()
 def test_interrupted_checkpoint_publication_is_recovered_in_wizard(self):
  self.first_failure();duplicate=self.output.parent/'.tv-setup-12345-0.tmp';os.link(self.checkpoint,duplicate);before=self.checkpoint.read_bytes();self.data.pop('failure');self.write_fixture()
  p=self.run_install([('Continue? Type yes:',b'yes\n'),('finish recovery?',b'yes\n'),('Choose the display:',b'1\n'),('Local name [',b'\n')]);self.assertEqual(p.returncode,0,p.stdout);self.assertFalse(duplicate.exists());self.assert_saved();self.assertTrue(before)
 def test_terminal_interrupt_then_identical_command_resumes_saved_target(self):
  p=self.run_install([('Continue? Type yes:',b'yes\n'),('Choose the display:',b'1\n'),('Local name [',b'\x03')]);self.assertNotEqual(p.returncode,0,p.stdout);self.assertFalse(self.output.exists());self.assertEqual(json.loads(self.checkpoint.read_text())['device'],'screen_192_0_2_10')
  p=self.run_install([('Continue? Type yes:',b'yes\n'),('Local name [',b'\n')]);self.assertEqual(p.returncode,0,p.stdout);self.assertNotIn('Choose the display:',p.stdout);self.assert_saved()
 def test_search_starts_directly_and_can_be_interrupted(self):
  p=self.run_install([('Continue? Type yes:',b'yes\n'),('Searching for TVs',b'\x03')]);self.assertNotEqual(p.returncode,0,p.stdout);self.assertNotIn('query this LAN',p.stdout);self.assertFalse(self.output.exists())
  p=self.run_install([('Continue? Type yes:',b'yes\n'),('Choose the display:',b'1\n'),('Local name [',b'Living room\n')]);self.assertEqual(p.returncode,0,p.stdout);self.assert_saved();self.assertEqual(json.loads(self.output.read_text())['devices']['screen']['display_name'],'Living room')
 @unittest.skipIf(LEGACY is None,'separate qualification with the verified v0.2.2 binary')
 def test_pinned_v022_uses_legacy_supported_arguments(self):
  self.env['TV_FLOW_BINARY']=str(LEGACY);old=self.h.asset('v0.2.2');args=['--version','v0.2.2','--setup','--prefix',str(self.h.prefix),'--archive',str(old)]
  p=self.h.run_terminal(args,[('Continue? Type yes:',b'yes\n'),('Choose the LAN interface',b'1\n'),('Choose the display:',b'1\n'),('Save this private configuration?',b'yes\n')],env=self.env,fragmented=True);self.assertEqual(p.returncode,0,p.stdout);self.assertTrue(self.output.exists());self.assertNotIn('unknown_onboarding_option',p.stdout)

if __name__=='__main__':unittest.main(defaultTest='InstallFlow')
