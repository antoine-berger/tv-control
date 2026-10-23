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
 def test_discovery_timeout_configuration_applies_to_discovery_and_setup(self):
  args=['--fixture',str(ROOT/'examples/discovery.json'),'--interface','lan-fixture','--json']
  p=self.run_cli('discover',*args,'--timeout-ms','10000');self.assertEqual(p.returncode,0,p.stdout);self.assertEqual(p.value['status'],'discovered')
  p=self.run_cli('setup',*self.base,'--json','--timeout-ms','10000');self.assertEqual(p.returncode,0,p.stdout);self.assertEqual(p.value['status'],'confirmation_required');self.assertFalse(self.output.exists())
  for timeout,code in [('10001','discovery_timeout_out_of_range'),('2000','discovery_timeout_out_of_range'),('1999','discovery_timeout_out_of_range'),('invalid','invalid_discovery_timeout')]:
   p=self.run_cli('discover',*args,'--timeout-ms',timeout);self.assertEqual(p.returncode,1,p.stdout);self.assertEqual(p.value['code'],code)
 def test_readable_names_match_text_json_and_selection_with_raw_diagnostics(self):
  data=json.loads((ROOT/'examples/discovery.json').read_text())
  opaque='adb-0123456789abcdef';server='Linux/2.6 UPnP/1.1 quick_ssdp/1.1'
  packet=bytes.fromhex(data['packets'][0]['hex']);data['packets'][0]['hex']=packet.replace(b'\x07Display',bytes([len(opaque)])+opaque.encode(),1).hex()
  data['packets'][2]['text']=f'HTTP/1.1 200 OK\r\nST: urn:dial-multiscreen-org:service:dial:1\r\nSERVER: {server}\r\nLOCATION: http://metadata.invalid/do-not-fetch\r\n\r\n'
  fixture=self.dir/'opaque-names.json';fixture.write_text(json.dumps(data));args=['--fixture',str(fixture),'--interface','lan-fixture']
  p=self.run_cli('discover',*args,'--json');self.assertEqual(p.returncode,0,p.stdout)
  candidates={c['address']:c for c in p.value['candidates']}
  for address,label,raw in [('192.0.2.10','Android device (ADB)',opaque),('192.0.2.30','Unidentified media device',server)]:
   self.assertEqual(candidates[address]['label'],label);self.assertEqual(candidates[address]['diagnostic_label'],raw)
  self.assertIn(opaque,candidates['192.0.2.10']['services'][0]['instance'])
  p=self.run_cli('discover',*args);self.assertEqual(p.returncode,0,p.stderr);listing=p.stdout
  p=self.run_cli('setup',*args,'--no-checkpoint','--adb',str(self.adb),'--curl',str(self.adb),'--output',str(self.output),input='1\n');self.assertEqual(p.returncode,0,p.stderr);self.assertTrue(self.output.exists());self.assertNotIn('Save this private configuration?',p.stderr)
  for text in [listing,p.stderr]:
   self.assertIn('Android device (ADB) (192.0.2.10)',text);self.assertIn('Unidentified media device (192.0.2.30)',text);self.assertNotIn(opaque,text);self.assertNotIn(server,text)
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
 def test_interactive_setup_saves_without_confirmation_and_refuses_overwrite(self):
  args=['--no-checkpoint','--fixture',str(ROOT/'examples/discovery.json'),'--adb',str(self.adb),'--curl',str(self.adb),'--output',str(self.output)]
  p=self.run_cli('setup',*args,input='');self.assertEqual(p.returncode,1);self.assertIn('setup_cancelled',p.stderr);self.assertFalse(self.output.exists())
  p=self.run_cli('setup',*args,input='1\n1\n');self.assertEqual(p.returncode,0,p.stderr);self.assertTrue(self.output.exists());self.assertNotIn('Save this private configuration?',p.stderr)
  before=self.output.read_bytes();p=self.run_cli('setup',*args,input='1\n1\n');self.assertEqual(p.returncode,1,p.stderr);self.assertIn('output_exists_use_resume_or_new_path',p.stderr);self.assertEqual(self.output.read_bytes(),before)
 def test_selected_dial_display_offers_guided_port_fallback(self):
  args=['--no-checkpoint','--fixture',str(ROOT/'examples/discovery.json'),'--adb',str(self.adb),'--curl',str(self.adb),'--output',str(self.output)]
  p=self.run_cli('setup',*args,input='1\n3\nmanual\n5555\nlegacy\n');self.assertEqual(p.returncode,0,p.stderr);self.assertTrue(self.output.exists())
  config=json.loads(self.output.read_text());self.assertEqual(config['devices']['screen']['endpoint'],'192.0.2.30:5555');self.assertIn('connection port',p.stderr)
 def test_selected_unresolved_display_does_not_use_pairing_port(self):
  data=json.loads((ROOT/'examples/discovery.json').read_text());data['packets'][0]['hex']=data['packets'][0]['hex'][:-2]+'14'
  fixture=self.dir/'unresolved.json';fixture.write_text(json.dumps(data))
  args=['--no-checkpoint','--fixture',str(fixture),'--adb',str(self.adb),'--curl',str(self.adb),'--output',str(self.output)]
  p=self.run_cli('setup',*args,input='1\n1\nmanual\n5555\nlegacy\n');self.assertEqual(p.returncode,0,p.stderr);self.assertTrue(self.output.exists())
  self.output.unlink()
  p=self.run_cli('setup',*args,input='1\n2\n');self.assertEqual(p.returncode,2,p.stderr);self.assertFalse(self.output.exists());self.assertIn('device setup is required',p.stdout)
 def test_empty_discovery_and_guided_manual_fallback(self):
  data=json.loads((ROOT/'examples/discovery.json').read_text());data['packets']=[];fixture=self.dir/'empty.json';fixture.write_text(json.dumps(data))
  p=self.run_cli('setup','--fixture',str(fixture),'--interface','lan-fixture','--json');self.assertEqual(p.value['status'],'no_devices_found')
  p=self.run_cli('setup','--no-checkpoint','--fixture',str(fixture),'--adb',str(self.adb),'--curl',str(self.adb),'--output',str(self.output),input='1\n192.0.2.10:5555\nlegacy\n');self.assertEqual(p.returncode,0,p.stderr);self.assertTrue(self.output.exists())
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
 def failing_fixture(self):
  data=json.loads((ROOT/'examples/discovery.json').read_text());data['failure']={'stage':'mdns_send','os_error':13};fixture=self.dir/'network-fixture.json';fixture.write_text(json.dumps(data));return fixture,data
 def guided_args(self,fixture):
  return ['--guided','--fixture',str(fixture),'--adb',str(self.adb),'--curl',str(self.adb),'--output',str(self.output)]
 def test_discovery_errno_is_one_json_error_without_raw_messages(self):
  fixture,_=self.failing_fixture();p=self.run_cli('discover','--fixture',str(fixture),'--interface','lan-fixture','--json');self.assertEqual(p.returncode,1);self.assertEqual(p.value['code'],'mdns_send_failed');self.assertEqual(p.value['os_error'],13);self.assertEqual(p.value['cause'],'not_established')
 def test_guided_resume_after_network_failure_and_existing_config_without_network(self):
  fixture,data=self.failing_fixture();args=self.guided_args(fixture)
  p=self.run_cli('setup',*args,input='\n');self.assertEqual(p.returncode,2,p.stderr);self.assertIn('OS error: 13',p.stderr);self.assertFalse(self.output.exists())
  checkpoint=self.output.with_name(self.output.name+'.setup-state.json');state=json.loads(checkpoint.read_text());self.assertEqual(state['interface'],'lan-fixture@192.0.2.1');self.assertEqual(stat.S_IMODE(checkpoint.stat().st_mode),0o600)
  data.pop('failure');fixture.write_text(json.dumps(data));p=self.run_cli('setup',*args,input='1\n\n');self.assertEqual(p.returncode,0,p.stderr);self.assertIn('Resuming local setup',p.stderr);self.assertNotIn('Choose the LAN interface',p.stderr);original=self.output.read_bytes()
  data['failure']={'stage':'mdns_send','os_error':13};fixture.write_text(json.dumps(data));p=self.run_cli('setup',*args);self.assertEqual(p.returncode,0,p.stderr);self.assertIn('saved configuration is valid',p.stdout);self.assertEqual(p.stderr,'');self.assertEqual(self.output.read_bytes(),original)
 def test_network_failure_offers_manual_fallback_in_same_wizard(self):
  fixture,_=self.failing_fixture();p=self.run_cli('setup',*self.guided_args(fixture),input='manual\n192.0.2.10:5555\nlegacy\n\n');self.assertEqual(p.returncode,0,p.stderr);self.assertEqual(json.loads(self.output.read_text())['devices']['screen']['endpoint'],'192.0.2.10:5555')
 def test_resume_revalidates_interface_and_preserves_corrupt_checkpoint(self):
  fixture,data=self.failing_fixture();args=self.guided_args(fixture);self.run_cli('setup',*args,input='\n');checkpoint=self.output.with_name(self.output.name+'.setup-state.json')
  data['interfaces'][0]['address']='192.0.2.2';fixture.write_text(json.dumps(data));p=self.run_cli('setup',*args,input='q\n');self.assertEqual(p.returncode,1);self.assertIn('saved interface/address changed',p.stderr);self.assertEqual(json.loads(checkpoint.read_text())['interface'],'lan-fixture@192.0.2.2');self.assertFalse(self.output.exists())
  checkpoint.write_text('{partial');p=self.run_cli('setup',*args);self.assertEqual(p.returncode,1);self.assertIn('Saved setup progress is unreadable',p.stderr);self.assertEqual(checkpoint.read_text(),'{partial');self.assertFalse(self.output.exists())
 def test_hard_discovery_timeout_is_not_an_os_error_and_offers_manual_fallback(self):
  fixture,data=self.failing_fixture();data['failure']={'stage':'deadline','os_error':None};fixture.write_text(json.dumps(data))
  p=self.run_cli('discover','--fixture',str(fixture),'--interface','lan-fixture','--json');self.assertEqual(p.returncode,1);self.assertEqual(p.value['status'],'timeout');self.assertNotIn('os_error',p.value)
  p=self.run_cli('setup',*self.guided_args(fixture),input='manual\n192.0.2.10:5555\nlegacy\nMy display\n');self.assertEqual(p.returncode,0,p.stderr);self.assertNotIn('OS error',p.stderr);self.assertIn('time limit',p.stderr);self.assertEqual(json.loads(self.output.read_text())['devices']['screen']['display_name'],'My display')
 def test_older_checkpoint_requires_choice_but_preserves_same_address_name(self):
  args=self.guided_args(ROOT/'examples/discovery.json')
  p=self.run_cli('setup',*args,input='1\nLiving room\n');self.assertEqual(p.returncode,0,p.stderr)
  self.output.unlink();checkpoint=self.output.with_name(self.output.name+'.setup-state.json')
  state=json.loads(checkpoint.read_text());state.pop('discovery_service');state['configuration_saved']=False;checkpoint.write_text(json.dumps(state))
  p=self.run_cli('setup',*args,input='1\n');self.assertEqual(p.returncode,0,p.stderr)
  self.assertIn('Choose the display:',p.stderr);self.assertNotIn('Local name [',p.stderr)
  self.assertEqual(json.loads(self.output.read_text())['devices']['screen']['display_name'],'Living room')
 def test_local_name_is_configuration_metadata_and_rejects_controls(self):
  p=self.run_cli('setup',*self.base,'--name','Living room','--json','--confirm');self.assertEqual(p.returncode,0,p.stdout);config=json.loads(self.output.read_text());self.assertEqual(config['devices']['screen']['display_name'],'Living room');self.assertEqual(config['devices']['screen']['endpoint'],'192.0.2.10:5555')
  self.output.unlink();p=self.run_cli('setup',*self.base,'--name','bad\nname','--json','--confirm');self.assertEqual(p.value['code'],'invalid_display_name');self.assertFalse(self.output.exists())
if __name__=='__main__':unittest.main()
