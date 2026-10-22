#!/usr/bin/env python3
"""Offline CLI onboarding contracts. No real interface, device, auth or tunnel calls."""
import json,os,pathlib,shlex,shutil,stat,subprocess,sys,tempfile,unittest
BINARY=str(pathlib.Path(sys.argv.pop(1)).resolve()) if len(sys.argv)>1 else str(pathlib.Path('target/release/tv-control').resolve())
ROOT=pathlib.Path(__file__).resolve().parent.parent
class Onboarding(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory(prefix="tv setup's ");self.dir=pathlib.Path(self.tmp.name);self.output=self.dir/"screen's config.local.json"
  self.adb=self.dir/'fake-adb';self.adb.write_text('#!/bin/sh\nexit 77\n');self.adb.chmod(0o700)
  self.base=['--fixture',str(ROOT/'examples/discovery.json'),'--interface','lan-fixture','--device','screen_192_0_2_10','--adb',str(self.adb),'--curl',str(self.adb),'--output',str(self.output)]
 def tearDown(self):self.tmp.cleanup()
 def run_cli(self,*args,input='',binary=None):
  p=subprocess.run([binary or BINARY,*args],input=input,text=True,capture_output=True,timeout=5)
  if '--json' in args:
   self.assertEqual(len(p.stdout.splitlines()),1,p.stdout);p.value=json.loads(p.stdout);self.assertEqual(p.stderr,'')
  return p
 def test_interfaces_and_discovery_use_offline_wire_fixture(self):
  p=self.run_cli('interfaces','--fixture',str(ROOT/'examples/discovery.json'),'--json');self.assertEqual(p.value['interfaces'][0]['name'],'lan-fixture')
  p=self.run_cli('discover','--fixture',str(ROOT/'examples/discovery.json'),'--interface','lan-fixture','--json');self.assertEqual(len(p.value['candidates']),3);self.assertTrue(p.value['simulation']);self.assertTrue(all(not c['control_verified'] for c in p.value['candidates']))
 def test_preview_write_resume_and_refuse_overwrite(self):
  p=self.run_cli('setup',*self.base,'--json');self.assertEqual(p.value['status'],'confirmation_required');self.assertFalse(self.output.exists());self.assertEqual(p.value['configuration']['apps'],{})
  p=self.run_cli('setup',*self.base,'--json','--confirm');self.assertEqual(p.returncode,0,p.stdout);self.assertEqual(p.value['status'],'configuration_saved');self.assertFalse(p.value['control_verified']);self.assertEqual(stat.S_IMODE(self.output.stat().st_mode),0o600)
  original=self.output.read_bytes();p=self.run_cli('setup',*self.base,'--json','--confirm');self.assertEqual(p.returncode,1);self.assertEqual(self.output.read_bytes(),original)
  p=self.run_cli('setup',*self.base,'--json','--confirm','--resume');self.assertEqual(p.value['status'],'already_configured')
  p=self.run_cli('setup',*self.base,'--json','--confirm','--resume','--server-port','15038');self.assertEqual(p.value['code'],'existing_config_differs_use_new_output');self.assertEqual(self.output.read_bytes(),original)
 def test_dry_run_cannot_write_or_scan(self):
  p=self.run_cli('setup',*self.base,'--json','--dry-run','--confirm');self.assertFalse(self.output.exists());self.assertEqual(p.value['status'],'confirmation_required')
  p=self.run_cli('setup','--json','--dry-run','--output',str(self.output));self.assertFalse(p.value['network_access']);self.assertFalse(self.output.exists())
  p=self.run_cli('discover','--json','--dry-run','--interface','does-not-exist');self.assertFalse(p.value['network_access'])
 def test_pairing_and_dial_only_do_not_create_control_config(self):
  for device,status in [('screen_192_0_2_20','pairing_required'),('screen_192_0_2_30','debugging_required')]:
   args=self.base.copy();args[args.index('--device')+1]=device;p=self.run_cli('setup',*args,'--json','--confirm');self.assertEqual(p.returncode,2);self.assertEqual(p.value['status'],status);self.assertFalse(self.output.exists())
 def test_interactive_confirmation_and_cancellation(self):
  args=['--fixture',str(ROOT/'examples/discovery.json'),'--adb',str(self.adb),'--curl',str(self.adb),'--output',str(self.output)]
  p=self.run_cli('setup',*args,input='1\n1\nno\n');self.assertEqual(p.returncode,2,p.stderr);self.assertFalse(self.output.exists());self.assertIn('Save this private configuration?',p.stderr)
  p=self.run_cli('setup',*args,input='');self.assertEqual(p.returncode,1);self.assertIn('setup_cancelled',p.stderr);self.assertFalse(self.output.exists())
  p=self.run_cli('setup',*args,input='1\n1\nyes\n');self.assertEqual(p.returncode,0,p.stderr);self.assertTrue(self.output.exists())
 def test_selected_dial_display_offers_guided_port_fallback(self):
  args=['--fixture',str(ROOT/'examples/discovery.json'),'--adb',str(self.adb),'--curl',str(self.adb),'--output',str(self.output)]
  p=self.run_cli('setup',*args,input='1\n3\nmanual\n5555\nlegacy\nyes\n');self.assertEqual(p.returncode,0,p.stderr);self.assertTrue(self.output.exists())
  config=json.loads(self.output.read_text());self.assertEqual(config['devices']['screen']['endpoint'],'192.0.2.30:5555');self.assertIn('connection port',p.stderr)
 def test_selected_unresolved_display_does_not_use_pairing_port(self):
  data=json.loads((ROOT/'examples/discovery.json').read_text());data['packets'][0]['hex']=data['packets'][0]['hex'][:-2]+'14'
  fixture=self.dir/'unresolved.json';fixture.write_text(json.dumps(data))
  args=['--fixture',str(fixture),'--adb',str(self.adb),'--curl',str(self.adb),'--output',str(self.output)]
  p=self.run_cli('setup',*args,input='1\n1\nmanual\n5555\nlegacy\nyes\n');self.assertEqual(p.returncode,0,p.stderr);self.assertTrue(self.output.exists())
  self.output.unlink()
  p=self.run_cli('setup',*args,input='1\n2\n');self.assertEqual(p.returncode,2,p.stderr);self.assertFalse(self.output.exists());self.assertIn('device setup is required',p.stdout)
 def test_empty_discovery_and_guided_manual_fallback(self):
  data=json.loads((ROOT/'examples/discovery.json').read_text());data['packets']=[];fixture=self.dir/'empty.json';fixture.write_text(json.dumps(data))
  p=self.run_cli('setup','--fixture',str(fixture),'--interface','lan-fixture','--json');self.assertEqual(p.value['status'],'no_devices_found')
  p=self.run_cli('setup','--fixture',str(fixture),'--adb',str(self.adb),'--curl',str(self.adb),'--output',str(self.output),input='1\n192.0.2.10:5555\nlegacy\nyes\n');self.assertEqual(p.returncode,0,p.stderr);self.assertTrue(self.output.exists())
 def test_json_requires_explicit_interface_and_selection(self):
  p=self.run_cli('discover','--fixture',str(ROOT/'examples/discovery.json'),'--json');self.assertEqual(p.value['code'],'interface_required')
  p=self.run_cli('setup','--fixture',str(ROOT/'examples/discovery.json'),'--interface','lan-fixture','--json');self.assertEqual(p.value['status'],'device_selection_required');self.assertEqual(p.returncode,2)
 def test_manual_fallback_cannot_target_another_subnet(self):
  p=self.run_cli('setup',*self.base,'--manual-endpoint','198.51.100.10:5555','--manual-kind','legacy','--json','--confirm');self.assertEqual(p.value['code'],'manual_endpoint_outside_selected_lan');self.assertFalse(self.output.exists())
 def test_connection_plan_quotes_paths_and_never_creates_connection(self):
  self.run_cli('setup',*self.base,'--json','--confirm')
  binary=self.dir/"controller's binary";shutil.copyfile(BINARY,binary);binary.chmod(0o700)
  p=self.run_cli('connect-chatgpt',str(self.output),'--json',binary=str(binary));self.assertFalse(p.value['executed']);self.assertEqual(p.value['status'],'manual_authorization_required')
  command=p.value['commands'][1];stdio=command[command.index('--mcp-command')+1];self.assertEqual(shlex.split(stdio),[str(binary),'mcp',str(self.output.resolve())])
  self.assertEqual(sorted(p.name for p in self.dir.iterdir()),["controller's binary",'fake-adb',"screen's config.local.json"])
 def test_doctor_offline_and_command_specific_flags(self):
  self.run_cli('setup',*self.base,'--json','--confirm');p=self.run_cli('doctor',str(self.output),'--json');self.assertEqual(p.value['status'],'configuration_valid');self.assertFalse(p.value['device_access'])
  p=self.run_cli('doctor',str(self.output),'--fixture',str(ROOT/'examples/discovery.json'),'--probe','--json');self.assertEqual(p.value['code'],'option_not_supported_for_command')
 def test_dependency_and_invalid_option_errors_are_structured(self):
  args=self.base.copy();args[args.index('--adb')+1]=str(self.dir/'missing');p=self.run_cli('setup',*args,'--json');self.assertEqual(p.value['code'],'adb_missing_install_official_platform_tools')
  p=self.run_cli('setup','--unknown','--json');self.assertEqual(p.value['code'],'unknown_onboarding_option')
if __name__=='__main__':unittest.main()
