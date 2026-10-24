#!/usr/bin/env python3
"""Real terminal + Rust MCP journey with private, loopback-only process fixtures.

The test supplies simulated ADB authorization, tunnel supervision and a cooperative
user copying a code. It does not establish access to a TV, account or hosted chat.
"""
import errno
import json
import os
import pathlib
import shlex
import signal
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

from journey_support.fixture import JourneyFixture, ROOT, TUNNEL, SECRET
from journey_support.runtime import Terminal

BINARY = pathlib.Path(sys.argv.pop(1)).resolve() if len(sys.argv) > 1 else pathlib.Path('target/debug/tv-control').resolve()


class CompleteJourney(JourneyFixture):
    binary = BINARY

    def test_mcp_failure_after_plugin_ui_stops_before_challenge(self):
        health=self.mcp_health_fixture()
        terminal=self.terminal();terminal.until(r'private ADB identity.*\[y/N\]');terminal.line('y')
        terminal.until('Tunnel ID:');terminal.line(TUNNEL)
        terminal.until(r'background TV controller.*\[y/N\]');terminal.line('y')
        self.enter_key(terminal)
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
        second=self.terminal();second.until('Resume this saved controller')
        self.assertEqual(self.runtime_state()['bridge_pid'],old['bridge_pid'])
        second.line('y');self.enter_key(second)
        self.decline_verification(second)
        self.assertEqual(second.wait(),0,second.text)
        current=self.runtime_state();self.assertNotEqual(current['bridge_pid'],old['bridge_pid'])
        self.assertEqual(current['alias'],old['alias']);self.assertEqual(current['tunnel_id'],old['tunnel_id'])
        self.assertEqual(self.config.read_bytes(),configuration)
        self.assertNotEqual((self.connection_parent/'connection/bridge-generation.json').read_bytes(),generation)
        self.assertIn('Controller updated and locally ready',second.text);self.assertNotIn('Verification code returned',second.text);self.assert_private_records()
        homes={event['home'] for event in map(json.loads,(self.root/'adb-events.jsonl').read_text().splitlines()) if '-L' in event['args']}
        self.assertEqual(len(homes),1)

    def test_declined_unhealthy_runtime_restart_preserves_running_controller(self):
        health=self.mcp_health_fixture()
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0)
        old=self.runtime_state();health.write_text('failed')
        before=(self.root/'adb-events.jsonl').read_bytes()
        second=self.terminal();second.until('Resume this saved controller');second.line('n')
        self.assertNotEqual(second.wait(),0,second.text)
        self.assertEqual(self.runtime_state()['bridge_pid'],old['bridge_pid'])
        self.assertEqual((self.root/'adb-events.jsonl').read_bytes(),before)
        self.assertNotIn('Runtime key',second.text);self.assertFalse(self.port_free())
        events=[json.loads(line)['args'] for line in (self.root/'tunnel-events.jsonl').read_text().splitlines()]
        self.assertFalse(any(args[:2]==['runtimes','stop'] for args in events))

    def test_complete_cooperative_fixture_requires_real_readonly_mcp_call(self):
        terminal=self.terminal();challenge=self.approve(terminal)
        self.assertNotIn('Ready.',terminal.text)
        code=self.channel().verify(challenge);self.assertRegex(code,r'^[0-9a-f]{32}$')
        terminal.line(code);self.assertEqual(terminal.wait(),0,terminal.text)
        self.assertIn('Ready.',terminal.text);self.assertNotIn(SECRET,terminal.text)
        self.assert_private_records()

    def test_minimum_output_budget_discovers_installer_proof_across_pages(self):
        config=json.loads(self.config.read_text());config['preferences']['max_output_bytes']=4096;self.config.write_text(json.dumps(config))
        terminal=self.terminal();challenge=self.approve(terminal)
        channel=self.channel();self.assertGreater(channel.discovery_pages,1)
        self.assertEqual(channel.discovery_names, {'tv_control','tv_capabilities','tv_read','tv_operation','tv_media_read','tv_verify_connection'})
        code=channel.verify(challenge);terminal.line(code)
        self.assertEqual(terminal.wait(),0,terminal.text);self.assertIn('Ready.',terminal.text)
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
        terminal.line(SECRET);terminal.until('Save this runtime key');terminal.line('n');terminal.until('Press Enter when the plugin is selected')
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
        second=self.terminal();self.preserve_runtime(second)
        self.assertEqual(self.runtime_state()['bridge_pid'],before)
        self.assertNotIn('Ready.',second.text);self.assert_private_records()

    def test_explicit_verification_resumes_pending_conversation_without_reinstall(self):
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0)
        before=self.runtime_state();terminal=self.verify_existing()
        self.assertEqual(self.runtime_state()['bridge_pid'],before['bridge_pid'])
        events=[json.loads(line)['args'] for line in (self.root/'tunnel-events.jsonl').read_text().splitlines()]
        self.assertEqual(sum(args[:2]==['runtimes','connect'] for args in events),1)
        self.assertFalse(any(args[:2]==['runtimes','stop'] for args in events))
        self.assertEqual(json.loads((self.connection_parent/'connection/progress.json').read_text())['outcome'],'verified')
        self.assert_private_records()

    def test_saved_key_restart_preserves_config_identity_and_tunnel_without_wizard(self):
        first=self.terminal();challenge=self.approve(first,persist=True)
        first.line(self.channel().verify(challenge));self.assertEqual(first.wait(),0,first.text)
        state=self.connection_parent/'connection';credential=state/'runtime-credential.json'
        self.assertEqual(credential.stat().st_mode&0o777,0o600)
        saved=credential.read_bytes();configuration=self.config.read_bytes();intent=(state/'runtime-intent.json').read_bytes()
        identity=state/'adb/adb-home/identity-sentinel';identity.write_text('fixture-stable-adb-identity');identity.chmod(0o600)
        old=self.runtime_state();generation=(state/'bridge-generation.json').read_bytes()
        second=self.terminal();second.until('Restart this owned controller');second.line('y')
        self.assertEqual(second.wait(),0,second.text);self.assert_maintenance_no_wizard(second)
        current=self.runtime_state();self.assertNotEqual(current['bridge_pid'],old['bridge_pid'])
        self.assertEqual(current['alias'],old['alias']);self.assertEqual(current['tunnel_id'],old['tunnel_id'])
        self.assertEqual(credential.read_bytes(),saved);self.assertEqual(self.config.read_bytes(),configuration)
        self.assertEqual((state/'runtime-intent.json').read_bytes(),intent)
        self.assertEqual(identity.read_text(),'fixture-stable-adb-identity')
        self.assertNotEqual((state/'bridge-generation.json').read_bytes(),generation)
        self.assertFalse((state/'verification/request.json').exists())
        self.assertFalse((state/'verification/response.json').exists())
        self.assertEqual(json.loads((state/'progress.json').read_text())['outcome'],'pending')
        self.assertIn('previous proof was invalidated',second.text)
        self.assert_private_records(allow_saved=True)
        self.verify_existing()
        self.assertEqual(json.loads((state/'progress.json').read_text())['outcome'],'verified')

    def test_environment_key_restarts_without_hidden_prompt_and_is_not_silently_saved(self):
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0)
        old=self.runtime_state();self.env['TV_CONTROL_TUNNEL_KEY']=SECRET
        second=self.terminal();second.until('Restart this owned controller');second.line('y')
        second.until('Save this runtime key');second.line('n')
        self.decline_verification(second)
        self.assertEqual(second.wait(),0,second.text);self.assert_maintenance_no_wizard(second)
        self.assertNotEqual(self.runtime_state()['bridge_pid'],old['bridge_pid'])
        self.assertFalse((self.connection_parent/'connection/runtime-credential.json').exists())
        self.assert_private_records()

    def test_stopped_saved_controller_resumes_without_reentering_saved_key(self):
        first=self.terminal();self.approve(first,persist=True);first.line('q');self.assertNotEqual(first.wait(),0)
        old=self.runtime_state();configuration=self.config.read_bytes()
        result=subprocess.run([str(BINARY),'disconnect',str(self.config),'--json'],env=self.env,capture_output=True,text=True,timeout=10)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr);self.assertTrue(self.port_free())
        second=self.terminal();second.until('Resume this saved controller');second.line('y')
        self.decline_verification(second)
        self.assertEqual(second.wait(),0,second.text);self.assert_maintenance_no_wizard(second)
        self.assertEqual(self.config.read_bytes(),configuration);self.assertEqual(self.runtime_state()['alias'],old['alias'])
        self.assertFalse(self.port_free());self.assert_private_records(allow_saved=True)

    def test_missing_migration_key_declined_preserves_controller_and_all_state(self):
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0)
        self.change_configuration();configuration=self.config.read_bytes()
        state=self.connection_parent/'connection';before={str(p.relative_to(state)):p.read_bytes() for p in state.rglob('*') if p.is_file()}
        old=self.runtime_state();second=self.terminal();second.until('Prepare a runtime key for this migration');second.line('n')
        self.assertEqual(second.wait(),0,second.text);self.assertNotIn('Runtime key (hidden',second.text)
        self.assertNotIn('Resume that verification now',second.text)
        self.assertEqual(self.config.read_bytes(),configuration);self.assertEqual(self.runtime_state()['bridge_pid'],old['bridge_pid'])
        after={str(p.relative_to(state)):p.read_bytes() for p in state.rglob('*') if p.is_file()}
        self.assertEqual(before,after);self.assertFalse(self.port_free())
        events=[json.loads(line)['args'] for line in (self.root/'tunnel-events.jsonl').read_text().splitlines()]
        self.assertFalse(any(args[:2]==['runtimes','stop'] for args in events))

    def test_cancel_missing_key_before_unhealthy_restart_keeps_existing_process(self):
        health=self.mcp_health_fixture();first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0)
        old=self.runtime_state();health.write_text('failed')
        second=self.terminal();second.until('Resume this saved controller');second.line('y')
        second.until('Runtime key .*hidden.*:');second.line('q')
        self.assertNotEqual(second.wait(),0,second.text);self.assertEqual(self.runtime_state()['bridge_pid'],old['bridge_pid'])
        self.assertFalse(self.port_free());self.assertFalse((self.connection_parent/'connection/runtime-credential.json').exists())
        events=[json.loads(line)['args'] for line in (self.root/'tunnel-events.jsonl').read_text().splitlines()]
        self.assertFalse(any(args[:2]==['runtimes','stop'] for args in events))

    def test_cancel_at_key_persistence_choice_does_not_stop_controller_or_save_key(self):
        health=self.mcp_health_fixture();first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0)
        old=self.runtime_state();health.write_text('failed')
        second=self.terminal();second.until('Resume this saved controller');second.line('y')
        second.until('Runtime key .*hidden.*:');second.line(SECRET)
        second.until('Save this runtime key');second.line('q')
        self.assertNotEqual(second.wait(),0,second.text);self.assertEqual(self.runtime_state()['bridge_pid'],old['bridge_pid'])
        self.assertFalse(self.port_free());self.assertFalse((self.connection_parent/'connection/runtime-credential.json').exists())
        self.assert_private_records()
        events=[json.loads(line)['args'] for line in (self.root/'tunnel-events.jsonl').read_text().splitlines()]
        self.assertFalse(any(args[:2]==['runtimes','stop'] for args in events))

    def test_same_onboard_command_offers_pending_first_conversation_verification(self):
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0)
        old=self.runtime_state();second=self.terminal()
        second.until('Configure a runtime key for future updates');second.line('n')
        second.until('Resume that verification now');second.line('y')
        second.until('Press Enter when the plugin is selected');second.line('')
        challenge=second.until(r'challenge ([0-9a-f]{32})').group(1).decode()
        second.until('Verification code returned');second.line(self.channel().verify(challenge))
        self.assertEqual(second.wait(),0,second.text)
        self.assertIn('Ready.',second.text);self.assertEqual(self.runtime_state()['bridge_pid'],old['bridge_pid'])
        for prompt in ['private ADB identity','background TV controller','Tunnel ID:', 'Runtime key (hidden']:
            self.assertNotIn(prompt,second.text)
        self.assert_private_records()

    def test_typed_code_without_an_mcp_call_is_not_evidence(self):
        terminal=self.terminal();self.approve(terminal)
        terminal.line('0'*32);self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertNotIn('Ready.',terminal.text);self.assert_private_records()

    def test_changed_configuration_stops_reapproves_and_reverifies(self):
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0,first.text)
        before=self.runtime_state()['bridge_pid']
        intent=self.connection_parent/'connection/runtime-intent.json';old=intent.read_bytes()
        self.change_configuration()
        second=self.terminal();self.prepare_migration(second);second.until('Stop the old controller and use the changed configuration');second.line('y')
        challenge=self.approve(second,saved=True)
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
        second=self.terminal();self.prepare_migration(second);second.until('Stop the old controller and use the changed configuration');second.line('n')
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
        second=self.terminal();self.prepare_migration(second);second.until('Stop the old controller and use the changed configuration');second.line('y')
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
        second=self.terminal();self.prepare_migration(second);second.until('Stop the old controller and use the changed configuration');second.line('y')
        second.until(r'private ADB identity.*\[y/N\]');second.line('q');self.assertNotEqual(second.wait(),0,second.text)
        revision=json.loads((state/'runtime-intent.json').read_text())['revision']
        # Emulate loss of the checkpoint write after atomic intent replacement.
        checkpoint.write_bytes(old_checkpoint)
        third=self.terminal();third.until('Resume this saved controller');third.line('y');self.enter_key(third)
        self.decline_verification(third)
        self.assertEqual(third.wait(),0,third.text)
        self.assertNotIn('Stop the old controller',third.text)
        self.assertEqual(json.loads((state/'bridge-generation.json').read_text())['revision'],revision)
        self.assertEqual(json.loads(checkpoint.read_text())['configuration_revision'],revision)
        self.assertEqual(json.loads(checkpoint.read_text())['outcome'],'pending')

    def test_configuration_change_requires_fresh_confirmation_that_runtime_stopped(self):
        first=self.terminal();self.approve(first);first.line('q');self.assertNotEqual(first.wait(),0,first.text)
        intent=self.connection_parent/'connection/runtime-intent.json';old=intent.read_bytes()
        (self.root/'lie-about-stop').write_text('fixture status must catch the still-running bridge')
        self.change_configuration()
        second=self.terminal();self.prepare_migration(second);second.until('Stop the old controller and use the changed configuration');second.line('y')
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

    def test_bridge_restart_after_code_requires_new_external_verification(self):
        terminal=self.terminal();challenge=self.approve(terminal)
        code=self.channel().verify(challenge)
        self.restart_fixture_runtime()
        terminal.line(code);self.assertNotEqual(terminal.wait(),0,terminal.text)
        self.assertIn('controller_restarted_repeat_verification',terminal.text)
        self.assertNotIn('Ready.',terminal.text)
        resumed=self.terminal();self.preserve_runtime(resumed)
        self.assertNotIn('Ready.',resumed.text)

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
        first=self.terminal(binary=first_binary,launcher=launcher);self.approve(first,persist=True);first.line('q')
        self.assertNotEqual(first.wait(),0,first.text);bridge_pid=self.runtime_state()['bridge_pid']
        pointer.write_text(str(second_binary))
        second=self.terminal(binary=second_binary,launcher=launcher)
        second.until('Restart this owned controller');second.line('y')
        self.decline_verification(second)
        self.assertEqual(second.wait(),0,second.text);self.assert_maintenance_no_wizard(second)
        self.assertNotEqual(self.runtime_state()['bridge_pid'],bridge_pid)
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
        terminal.line(self.channel().verify(challenge));terminal.until('Search for other devices');terminal.line('n');self.assertEqual(terminal.wait(15),0,terminal.text)
        self.assertIn('Ready.',terminal.text);self.assertNotIn('Choose the display',terminal.text)
        self.assertEqual(len(downloads.read_text().splitlines()),2)
        self.assertTrue((installer.prefix/'tv-control').is_file());self.assert_private_records()
        saved_configuration=self.config.read_bytes()
        old=self.runtime_state()
        rerun=Terminal(['-s'],env,binary=pathlib.Path('/bin/sh'),script=(ROOT/'install.sh').read_bytes())
        self.terminals.append(rerun)
        rerun.until('Continue\\? Type yes:');rerun.line('yes')
        rerun.until('Configure a runtime key for future updates');rerun.line('n')
        rerun.until('Search for other devices');rerun.line('n')
        self.assertEqual(rerun.wait(15),0,rerun.text);self.assert_maintenance_no_wizard(rerun)
        self.assertNotIn('Choose the display',rerun.text);self.assertNotIn('Ready.',rerun.text)
        self.assertEqual(self.runtime_state()['bridge_pid'],old['bridge_pid'])
        self.assertEqual(self.config.read_bytes(),saved_configuration)
        self.assertEqual(len(downloads.read_text().splitlines()),2)
        # A second installation shares HOME and the configuration but has never
        # owned this controller. Its uninstall must not even query the tunnel.
        other_prefix=installer.root/'another installation'
        archive=installer.asset()
        result=installer.run_script(['--prefix',str(other_prefix),'--version','v0.4.8','--archive',str(archive),'--json'],env=env)
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

    def test_sigint_discards_partial_hidden_key_before_returning_to_parent(self):
        self.assert_secret_cancellation_discards_unread_input(signal.SIGINT)

    def test_sigterm_discards_partial_hidden_key_before_returning_to_parent(self):
        self.assert_secret_cancellation_discards_unread_input(signal.SIGTERM)


class FixtureCleanup(unittest.TestCase):
    def fixture(self):
        fixture=CompleteJourney()
        fixture.channels=[];fixture.terminals=[];fixture.installers=[]
        directory=tempfile.TemporaryDirectory();self.addCleanup(directory.cleanup)
        fixture.root=pathlib.Path(directory.name);fixture.temp=mock.Mock()
        fixture.port_free=mock.Mock(return_value=True)
        fixture.runtime_state=mock.Mock(return_value={'supervisor_pid':12345,'bridge_pid':12346})
        (fixture.root/'supervisor-12345.stopped').touch()
        supervisor=mock.Mock(pid=12345,returncode=None)
        fixture.fixture_supervisors=[supervisor]
        return fixture,supervisor

    def test_reaps_owned_supervisor_without_signalling_zombie_groups(self):
        fixture,supervisor=self.fixture()
        # Reproduce Darwin EPERM if any stale/zombie group is signalled.
        with mock.patch.object(os,'kill',side_effect=AssertionError('stale PID')) as terminate, mock.patch.object(os,'killpg',side_effect=PermissionError(errno.EPERM,'zombie group')) as kill_group:
            fixture.tearDown()
        terminate.assert_not_called();kill_group.assert_not_called()
        supervisor.wait.assert_called_once_with(timeout=1);supervisor.kill.assert_not_called()
        self.assertTrue((fixture.root/'stop-fixture').exists())
        fixture.temp.cleanup.assert_called_once()

    def test_real_exited_child_is_reaped_without_a_signal(self):
        fixture,_=self.fixture()
        child=subprocess.Popen([sys.executable,'-c','pass'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        fixture.fixture_supervisors=[child]
        try:
            with mock.patch.object(os,'kill',side_effect=AssertionError('already exited child')):
                fixture.tearDown()
            self.assertEqual(child.returncode,0)
        finally:
            if child.poll() is None:child.kill()
            child.wait(timeout=5)

    def test_reused_nonchild_runtime_pids_are_never_signalled(self):
        fixture,_=self.fixture();fixture.fixture_supervisors=[]
        with mock.patch.object(os,'kill') as terminate, mock.patch.object(os,'killpg') as kill_group:
            fixture.tearDown()
        terminate.assert_not_called();kill_group.assert_not_called()

    def test_owned_child_permission_error_is_not_suppressed(self):
        fixture,supervisor=self.fixture()
        supervisor.wait.side_effect=subprocess.TimeoutExpired('fixture supervisor',1)
        supervisor.kill.side_effect=PermissionError(errno.EPERM,'owned child denied')
        with self.assertRaises(PermissionError):fixture.tearDown()
        supervisor.kill.assert_called_once_with()

    def test_unresponsive_owned_supervisor_is_still_escalated_and_reaped(self):
        fixture,supervisor=self.fixture()
        supervisor.wait.side_effect=[subprocess.TimeoutExpired('fixture supervisor',1),0]
        fixture.tearDown();supervisor.kill.assert_called_once_with()
        self.assertEqual(supervisor.wait.call_args_list,[mock.call(timeout=1),mock.call(timeout=5)])

    def test_listener_leak_still_fails_after_supervisor_reaped(self):
        fixture,_=self.fixture();fixture.port_free.return_value=False
        with mock.patch.object(time,'monotonic',side_effect=[0,7,8,11]):
            with self.assertRaisesRegex(AssertionError,'owned ADB listener leaked'):fixture.tearDown()
        self.assertTrue((fixture.root/'stop-adb-fixture').exists())
        fixture.temp.cleanup.assert_called_once()

    def test_missing_supervisor_cleanup_acknowledgement_fails(self):
        fixture,_=self.fixture();(fixture.root/'supervisor-12345.stopped').unlink()
        with mock.patch.object(time,'monotonic',side_effect=[0,7]):
            with self.assertRaisesRegex(AssertionError,'supervisor did not finish'):fixture.tearDown()


if __name__ == '__main__': unittest.main()
