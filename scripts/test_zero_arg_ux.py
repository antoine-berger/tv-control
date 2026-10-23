#!/usr/bin/env python3
"""Zero-argument installer UX under a PTY; synthetic assets and wire fixtures only.

Usage: python3 scripts/test_zero_arg_ux.py target/release/tv-control [-v]
The long human-delay regression takes about 40 seconds. No production switches,
UDP sockets, ADB execution, authentication, pairing or user installations are used.
"""
import json
import signal
import sys
import unittest

# The existing flow module consumes its binary/optional legacy arguments. Keep
# unittest selectors out of that parser while reusing its native fixture launcher.
if len(sys.argv)<2:
 raise SystemExit(__doc__)
_args=sys.argv[:]
sys.argv=sys.argv[:2]
from test_install_flow import InstallFlow
sys.argv=_args[:1]+_args[2:]


class ZeroArgumentUX(InstallFlow):
 def setUp(self):
  super().setUp()
  self.adb_log=self.h.root/'adb-invocations'
  self.env['TV_UX_ADB_LOG']=str(self.adb_log)
  (self.h.bin/'adb').write_text('#!/bin/sh\nprintf invoked >> "$TV_UX_ADB_LOG"\nexit 77\n')

 def terminal(self,steps,timeout=20):
  return self.h.run_terminal([],steps,env=self.env,fragmented=True,timeout=timeout)

 def finish(self,selection=True):
  steps=[('Continue? Type yes:',b'yes\n')]
  if selection:steps.append(('Choose the display:',b'1\n'))
  return steps+[('Local name [',b'\n')]

 def assert_offline(self):
  self.assertFalse(self.adb_log.exists(),'setup executed ADB')
  self.assertEqual(self.downloads.read_text().splitlines(),['download','download'],
                   'unexpected executable access or repeated archive downloads')

 def assert_no_discovery_confirmation(self,p):
  self.assertIn('Searching for TVs',p.stdout)
  before=p.stdout.split('Searching for TVs',1)[0]
  self.assertEqual(before.count('Type yes:'),1,before)
  self.assertNotIn('query this LAN',p.stdout)
  self.assertNotIn('Find TVs on',p.stdout)
  self.assertNotIn('Save this private configuration?',p.stdout)

 def test_single_interface_starts_discovery_without_another_yes(self):
  p=self.terminal(self.finish())
  self.assertEqual(p.returncode,0,p.stdout)
  self.assert_no_discovery_confirmation(p)
  self.assertNotIn('Choose the LAN interface',p.stdout)
  self.assert_saved();self.assert_offline()

 def test_long_human_waits_do_not_spend_the_network_budget(self):
  self.data['interfaces'].append(dict(name='other-fixture',address='198.51.100.1',netmask='255.255.255.0'))
  self.write_fixture()
  p=self.terminal([
   ('Continue? Type yes:',b'yes\n'),
   ('Choose a number:',b'1\n',31),
   ('Choose the display:',b'1\n',3),
   ('Local name [',b'Living room\n',3),
  ],timeout=65)
  self.assertGreaterEqual(p.response_delays[1],31)
  self.assertGreaterEqual(p.response_delays[2],3)
  self.assertGreaterEqual(p.response_delays[3],3)
  self.assertEqual(p.returncode,0,p.stdout)
  self.assertNotIn('time limit',p.stdout)
  self.assertNotIn('discovery_expired',p.stdout)
  self.assert_no_discovery_confirmation(p)
  self.assert_saved();self.assert_offline()

 def test_hard_deadline_is_distinct_from_udp_failure_and_rerun_recovers(self):
  self.data['failure']={'stage':'deadline','os_error':None};self.write_fixture()
  p=self.terminal([('Continue? Type yes:',b'yes\n'),('Type manual, or Enter to stop:',b'\n')])
  self.assertEqual(p.returncode,2,p.stdout)
  self.assertIn('time limit',p.stdout);self.assertNotIn('OS error',p.stdout)
  self.assertFalse(self.output.exists());self.assertTrue(self.checkpoint.exists())
  self.assert_no_discovery_confirmation(p)
  self.data.pop('failure');self.write_fixture()
  p=self.terminal(self.finish())
  self.assertEqual(p.returncode,0,p.stdout)
  self.assertIn('Resuming local setup',p.stdout)
  self.assert_saved();self.assert_offline()

 def test_udp_failures_keep_errno_checkpoint_and_configuration_absent(self):
  for stage,code in [('socket','discovery_socket_failed'),('mdns_send','mdns_send_failed'),
                     ('ssdp_send','ssdp_send_failed'),('receive','discovery_receive_failed')]:
   with self.subTest(stage=stage):
    self.data['failure']={'stage':stage,'os_error':13};self.write_fixture()
    p=self.terminal([('Continue? Type yes:',b'yes\n'),('Type manual, or Enter to stop:',b'\n')])
    self.assertEqual(p.returncode,2,p.stdout)
    self.assertIn(code,p.stdout);self.assertIn('OS error: 13',p.stdout)
    self.assertIn('cause is not established',p.stdout)
    self.assertFalse(self.output.exists())
    self.assertEqual(self.checkpoint.stat().st_mode&0o777,0o600)
    self.assertEqual(json.loads(self.checkpoint.read_text())['interface'],'lan-fixture@192.0.2.1')
    self.assert_no_discovery_confirmation(p);self.assert_offline()

 def test_no_results_offers_manual_entry_and_cancellation_writes_no_config(self):
  self.data['packets']=[];self.write_fixture()
  p=self.terminal([('Continue? Type yes:',b'yes\n'),("Enter the TV's connection IPv4:port",b'q\n')])
  self.assertNotEqual(p.returncode,0,p.stdout)
  self.assertNotIn('Choose the display:',p.stdout)
  self.assertNotIn('Save this private configuration?',p.stdout)
  self.assertFalse(self.output.exists());self.assertTrue(self.checkpoint.exists())
  self.assert_no_discovery_confirmation(p);self.assert_offline()

 def test_sigkill_before_publication_then_identical_installer_resumes(self):
  p=self.terminal(self.finish()[:-1]+[('Local name [',signal.SIGKILL)])
  self.assertNotEqual(p.returncode,0,p.stdout)
  self.assertFalse(self.output.exists())
  state=json.loads(self.checkpoint.read_text())
  self.assertEqual(state['device'],'screen_192_0_2_10')
  self.assertFalse(state['configuration_saved'])
  self.assertEqual(self.checkpoint.stat().st_mode&0o777,0o600)
  p=self.terminal([('Continue? Type yes:',b'yes\n'),('Local name [',b'\n')])
  self.assertEqual(p.returncode,0,p.stdout)
  self.assertNotIn('Choose the display:',p.stdout)
  self.assert_saved();self.assert_offline()

 def test_completed_rerun_does_not_even_parse_the_discovery_fixture(self):
  p=self.terminal(self.finish());self.assertEqual(p.returncode,0,p.stdout)
  before=self.output.read_bytes();checkpoint=self.checkpoint.read_bytes()
  self.fixture.write_text('{unreadable discovery fixture')
  p=self.terminal([('Continue? Type yes:',b'yes\n')])
  self.assertEqual(p.returncode,0,p.stdout)
  self.assertIn('saved configuration is valid',p.stdout)
  self.assertNotIn('Searching for TVs',p.stdout)
  self.assertEqual(self.output.read_bytes(),before)
  self.assertEqual(self.checkpoint.read_bytes(),checkpoint)
  self.assert_offline()

 def short_ttl(self):
  packet=self.data['packets'][0]
  self.assertEqual(packet['hex'].count('800100000078'),2)
  packet['hex']=packet['hex'].replace('800100000078','800100000001')

 def test_expiry_while_naming_refreshes_and_keeps_name_with_changed_port(self):
  self.short_ttl()
  self.data['refresh_packets']=json.loads(json.dumps(self.data['packets']))
  packet=self.data['refresh_packets'][0]
  self.assertIn('0000000015b3',packet['hex'])
  packet['hex']=packet['hex'].replace('0000000015b3','000000002329')
  self.write_fixture()
  p=self.terminal(self.finish()[:-1]+[('Local name [',b'Living room\n',2)])
  self.assertEqual(p.returncode,0,p.stdout)
  self.assertIn('Refreshing its endpoint',p.stdout)
  self.assertNotIn('Save this private configuration?',p.stdout)
  device=json.loads(self.output.read_text())['devices']['screen']
  self.assertEqual(device['endpoint'],'192.0.2.10:9001')
  self.assertEqual(device['display_name'],'Living room')
  self.assertEqual(json.loads(self.checkpoint.read_text())['display_name'],'Living room')
  self.assert_saved();self.assert_offline()

 def test_expiry_during_selection_refreshes_without_repeating_choice(self):
  self.short_ttl();self.write_fixture()
  p=self.terminal([('Continue? Type yes:',b'yes\n'),('Choose the display:',b'1\n',2),('Local name [',b'\n')])
  self.assertEqual(p.returncode,0,p.stdout)
  self.assertEqual(p.stdout.count('Choose the display:'),1)
  self.assertIn('Refreshing its endpoint',p.stdout)
  self.assert_saved();self.assert_offline()

 def test_disappeared_display_offers_retry_and_retains_name_for_resume(self):
  self.short_ttl();self.data['refresh_packets']=[];self.write_fixture()
  p=self.terminal(self.finish()[:-1]+[('Local name [',b'Living room\n',2),('Enter to retry, or q to stop:',b'\n'),('Enter to retry, or q to stop:',b'q\n')])
  self.assertNotEqual(p.returncode,0,p.stdout)
  self.assertEqual(p.stdout.count('Refreshing its endpoint'),2)
  self.assertIn('selected_display_not_found',p.stdout)
  self.assertFalse(self.output.exists());state=json.loads(self.checkpoint.read_text())
  self.assertEqual(state['display_name'],'Living room')
  self.assertEqual(state['device'],'screen_192_0_2_10');self.assertFalse(state['configuration_saved'])
  self.assertTrue(state['discovery_service'])
  self.data.pop('refresh_packets');self.write_fixture()
  p=self.terminal([('Continue? Type yes:',b'yes\n')])
  self.assertEqual(p.returncode,0,p.stdout)
  self.assertNotIn('Choose the display:',p.stdout);self.assertNotIn('Local name [',p.stdout)
  self.assertEqual(json.loads(self.output.read_text())['devices']['screen']['display_name'],'Living room')
  self.assert_saved();self.assert_offline()

 def test_replacement_at_same_address_is_never_selected_silently(self):
  self.short_ttl()
  self.data['refresh_packets']=json.loads(json.dumps(self.data['packets']))
  packet=self.data['refresh_packets'][0]
  packet['hex']=packet['hex'].replace('07446973706c6179','07416e6f74686572')
  self.write_fixture()
  p=self.terminal(self.finish()[:-1]+[('Local name [',b'Living room\n',2),('Enter to retry, or q to stop:',b'q\n')])
  self.assertNotEqual(p.returncode,0,p.stdout);self.assertIn('selected_display_identity_changed',p.stdout)
  self.assertFalse(self.output.exists());self.assert_offline()
  self.data['packets']=self.data.pop('refresh_packets');self.write_fixture()
  p=self.terminal([('Continue? Type yes:',b'yes\n'),('Choose the display:',b'q\n')])
  self.assertNotEqual(p.returncode,0,p.stdout)
  self.assertIn('same identity',p.stdout);self.assertFalse(self.output.exists())


if __name__=='__main__':
 # Existing flow cases are inherited for helpers, but execute only this delta.
 unittest.main(defaultTest=['ZeroArgumentUX.'+name for name in ZeroArgumentUX.__dict__ if name.startswith('test_')])
