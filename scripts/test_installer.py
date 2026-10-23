#!/usr/bin/env python3
"""Installer contracts: synthetic local assets and temporary prefixes only, never HOME installs."""
import hashlib,io,json,os,pathlib,shutil,subprocess,sys,tarfile,tempfile,unittest,platform,struct
import pty,select,signal,threading,time
ROOT=pathlib.Path(__file__).resolve().parent.parent
BASE=['tv-control','LICENSE','README.md','PLAN.md','examples/config.json','examples/mcp.json','examples/discovery.json','benchmarks/README.md','benchmarks/host-macos-arm64.json']
BASE+=['docs/'+n+'.md' for n in ['ACCEPTANCE','ADAPTERS','ADR','CLI','CONFIGURATION','DEVELOPMENT','MCP','ONBOARDING','REVIEW','ROUTER']]
SHELLS=[['/bin/sh']]
if shutil.which('dash'):SHELLS.append([shutil.which('dash')])
if shutil.which('bash'):SHELLS.append([shutil.which('bash'),'--posix'])
if shutil.which('busybox'):SHELLS.append([shutil.which('busybox'),'sh'])
HOST_ARCH='aarch64' if platform.machine() in ('arm64','aarch64') else 'x86_64'
HOST_TARGET=HOST_ARCH+('-apple-darwin' if platform.system()=='Darwin' else '-linux-musl')
def header(target):
 data=bytearray(64)
 if 'linux-musl' in target:
  data[:6]=b'\x7fELF\x02\x01';struct.pack_into('<HH',data,16,3,183 if target.startswith('aarch64') else 62)
 else:
  data[:4]=b'\xcf\xfa\xed\xfe';struct.pack_into('<I',data,4,0x0100000c if target.startswith('aarch64') else 0x01000007);struct.pack_into('<I',data,12,2)
 return bytes(data)
class Installer(unittest.TestCase):
 @classmethod
 def setUpClass(cls):
  cls.compiler_tmp=tempfile.TemporaryDirectory(prefix='tv-native-fixture-');root=pathlib.Path(cls.compiler_tmp.name)
  source=root/'stub.c';source.write_text(r'''#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
int main(int argc,char **argv) {
 if(argc>1 && strcmp(argv[1],"--version")==0) {printf("fixture:%s\n",argv[0]);return 0;}
 if(argc>1 && strcmp(argv[1],"setup")==0) {
  const char *once=getenv("TV_TEST_SETUP_FAIL_ONCE");if(once && access(once,F_OK)) {FILE*f=fopen(once,"w");if(!f)return 11;fclose(f);fputs("fixture discovery failure\n",stderr);return 31;}
  if(getenv("TV_TEST_SETUP_INPUT")) {char answer[64];if(!isatty(0))return 9;fputs("Wizard answer: ",stderr);fflush(stderr);if(!fgets(answer,sizeof(answer),stdin)||strcmp(answer,"wizard-answer\n"))return 10;}
  const char *p=getenv("TV_TEST_SETUP_MARKER");if(!p)return 7;FILE*f=fopen(p,"w");if(!f)return 8;fputs("setup\n",f);return fclose(f);
 }
 return 0;
}
''');binary=root/'stub';subprocess.run(['cc',str(source),'-o',str(binary)],check=True,capture_output=True);cls.native_stub=binary.read_bytes()
 @classmethod
 def tearDownClass(cls):cls.compiler_tmp.cleanup()
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory(prefix="installer test's ");self.root=pathlib.Path(self.tmp.name).resolve();self.prefix=self.root/"app's files"
  self.bin=self.root/'commands';self.bin.mkdir();uname=self.bin/'uname';uname.write_text('#!/bin/sh\ncase "$1" in -s) printf "%s\\n" "$TV_TEST_OS";; -m) printf "%s\\n" "$TV_TEST_ARCH";; esac\n');uname.chmod(0o700)
  self.env=os.environ.copy();self.env.update(PATH=str(self.bin)+os.pathsep+os.environ['PATH'],TV_TEST_OS=platform.system(),TV_TEST_ARCH=platform.machine(),XDG_CONFIG_HOME=str(self.root/'config'))
 def tearDown(self):self.tmp.cleanup()
 def asset(self,version='v0.3.0',target=HOST_TARGET,mutation=None,binary_target=None):
  folder=self.root/(version+'-'+target+('-'+mutation if mutation else ''));folder.mkdir(exist_ok=True);archive=folder/('tv-control-'+target+'.tar.gz')
  with tarfile.open(archive,'w:gz') as bundle:
   for name in BASE:
    if name=='tv-control':
     actual=binary_target or target;data=self.native_stub if actual==HOST_TARGET else header(actual)
    else:data=b'fixture\n'
    info=tarfile.TarInfo(name);info.mode=0o755 if name=='tv-control' else 0o644;info.size=len(data);bundle.addfile(info,io.BytesIO(data))
   if mutation:
    info=tarfile.TarInfo({'traversal':'../escape','absolute':'/tmp/installer-escape','symlink':'docs/evil','hardlink':'docs/hard','duplicate':'README.md','extra':'private.local.json'}[mutation]);info.size=1
    if mutation=='symlink':info.type=tarfile.SYMTYPE;info.linkname='/tmp/outside';info.size=0
    if mutation=='hardlink':info.type=tarfile.LNKTYPE;info.linkname='README.md';info.size=0
    bundle.addfile(info,io.BytesIO(b'x') if info.size else None)
  checksum=archive.with_suffix(archive.suffix+'.sha256');checksum.write_text(hashlib.sha256(archive.read_bytes()).hexdigest()+'  '+archive.name+'\n');return archive
 def run_script(self,args,sh=None,input='',env=None):
  p=subprocess.run([*(sh or SHELLS[0]),str(ROOT/'install.sh'),*args],input=input,text=True,capture_output=True,env=env or self.env,timeout=15,start_new_session=True)
  if '--json' in args:
   self.assertEqual(len(p.stdout.splitlines()),1,p.stdout+p.stderr);p.value=json.loads(p.stdout)
  return p
 def run_terminal(self,args,responses,sh=None,env=None,fragmented=False,timeout=20):
  """Controlling PTY with ordered (prompt, bytes-or-signal[, human-delay]) steps.

  Delays start when a fresh prompt appears and do not stop draining the PTY.
  Script stdin remains a separate pipe, never keyboard input. Integer actions
  signal the entire test process group, including the installer and wizard.
  """
  shell=sh or SHELLS[0];read_fd,write_fd=os.pipe();pid,master=pty.fork()
  if pid==0:
   os.close(write_fd);os.dup2(read_fd,0);os.close(read_fd)
   command=[*shell,'-s','--',*args] if fragmented else [*shell,str(ROOT/'install.sh'),*args]
   os.execvpe(command[0],command,env or self.env)
  os.close(read_fd);feed_errors=[];prefix_wait=threading.Event();remainder_started=threading.Event()
  def feed():
   try:
    if fragmented:
     data=(ROOT/'install.sh').read_bytes();cut=data.index(b'# Parent creation')
     os.write(write_fd,data[:cut]);prefix_wait.set();time.sleep(0.15);remainder_started.set()
     for offset in range(cut,len(data),71):
      os.write(write_fd,data[offset:offset+71]);time.sleep(0.001)
    else:os.write(write_fd,b'no\nscript stdin is not confirmation input\n')
   except BrokenPipeError:pass
   except BaseException as error:feed_errors.append(error)
   finally:os.close(write_fd)
  feeder=threading.Thread(target=feed,daemon=True);feeder.start();output=b'';index=0;status=None;deadline=time.monotonic()+timeout;cursor=0;pending=None;delays=[]
  try:
   while time.monotonic()<deadline:
    if select.select([master],[],[],0.02)[0]:
     try:chunk=os.read(master,65536)
     except OSError:chunk=b''
     output+=chunk
     if fragmented and prefix_wait.is_set() and not remainder_started.is_set():
      self.assertNotIn(b'Continue? Type yes:',output,'installer executed a partial script')
    if pending is None and index<len(responses):
     step=responses[index];needle=step[0].encode();found=output.find(needle,cursor)
     if found>=0:
      delay=step[2] if len(step)>2 else 0
      if delay<0:raise ValueError('PTY response delay must be nonnegative')
      pending=(time.monotonic(),delay);cursor=found+len(needle)
    if pending is not None and time.monotonic()-pending[0]>=pending[1]:
     delays.append(time.monotonic()-pending[0]);action=responses[index][1]
     if isinstance(action,int):os.killpg(pid,action)
     else:os.write(master,action)
     index+=1;pending=None
    waited,wait_status=os.waitpid(pid,os.WNOHANG)
    if waited:
     status=wait_status
     # Reap only after collecting the final messages, even for fast exits.
     while select.select([master],[],[],0.02)[0]:
      try:chunk=os.read(master,65536)
      except OSError:break
      if not chunk:break
      output+=chunk
     break
   self.assertIsNotNone(status,'PTY installer timeout: '+output.decode(errors='replace'))
   self.assertEqual(index,len(responses),output.decode(errors='replace'))
  finally:
   if status is None:
    os.killpg(pid,signal.SIGKILL);os.waitpid(pid,0)
   feeder.join(timeout=2);os.close(master)
  self.assertFalse(feeder.is_alive());self.assertFalse(feed_errors,feed_errors)
  result=subprocess.CompletedProcess(args,os.waitstatus_to_exitcode(status),output.decode(errors='replace'),'');result.response_delays=delays;return result
 def install(self,archive,version='v0.3.0',extra=(),sh=None):return self.run_script(['--prefix',str(self.prefix),'--version',version,'--archive',str(archive),'--json',*extra],sh=sh)
 def test_all_shells_install_launch_update_and_uninstall_without_user_config_removal(self):
  old=self.asset('v0.2.0');new=self.asset()
  for sh in SHELLS:
   with self.subTest(shell=sh):
    p=self.install(old,'v0.2.0',sh=sh);self.assertEqual(p.returncode,0,p.stdout+p.stderr);self.assertEqual(p.value['status'],'installed');self.assertEqual(self.prefix.stat().st_mode&0o777,0o700)
    p=subprocess.run([str(self.prefix/'tv-control'),'--version'],text=True,capture_output=True,env=self.env);self.assertIn('/versions/v0.2.0/tv-control',p.stdout)
    p=self.install(new,extra=['--update'],sh=sh);self.assertEqual(p.returncode,0,p.stdout+p.stderr);self.assertEqual((self.prefix/'current').read_text(),'v0.3.0\n');self.assertTrue((self.prefix/'versions/v0.2.0').is_dir())
    marker=self.root/'user-config.local.json';marker.write_text('keep')
    p=self.run_script(['--prefix',str(self.prefix),'--uninstall','--json'],sh=sh);self.assertEqual(p.returncode,0,p.stdout+p.stderr);self.assertFalse(self.prefix.exists());self.assertEqual(marker.read_text(),'keep')
    self.assertFalse(list(self.root.glob('.tv-control-stage.*')));self.assertFalse(list(self.root.glob('.*.install-lock')))
 def test_platform_asset_mapping_for_all_supported_hosts(self):
  for system,machine,target in [('Darwin','arm64','aarch64-apple-darwin'),('Linux','aarch64','aarch64-linux-musl'),('Linux','x86_64','x86_64-linux-musl')]:
   env=self.env.copy();env.update(TV_TEST_OS=system,TV_TEST_ARCH=machine);archive=self.asset(target=target)
   p=self.run_script(['--prefix',str(self.prefix),'--archive',str(archive),'--json'],env=env);self.assertEqual(p.returncode,0,p.stdout+p.stderr)
   p=self.run_script(['--prefix',str(self.prefix),'--uninstall','--json'],env=env);self.assertEqual(p.returncode,0,p.stdout+p.stderr)
 def test_macos_intel_is_rejected_before_download_setup_or_writes(self):
  env=self.env.copy();env.update(TV_TEST_OS='Darwin',TV_TEST_ARCH='x86_64');log=self.root/'downloads'
  curl=self.bin/'curl';curl.write_text('#!/bin/sh\nprintf called >> "$TV_TEST_CURL_LOG"\nexit 99\n');curl.chmod(0o700);env['TV_TEST_CURL_LOG']=str(log)
  for extra in [[],['--dry-run']]:
   p=self.run_script(['--prefix',str(self.prefix),'--json',*extra],env=env);self.assertEqual(p.returncode,1,p.stdout+p.stderr);self.assertEqual(p.value['code'],'macos_intel_unsupported');self.assertFalse(self.prefix.exists());self.assertFalse(log.exists())
  archive=self.asset();self.assertEqual(self.install(archive).returncode,0);before={p.relative_to(self.prefix):p.read_bytes() for p in self.prefix.rglob('*') if p.is_file()};marker=self.root/'setup-marker';env['TV_TEST_SETUP_MARKER']=str(marker)
  p=self.run_terminal(['--prefix',str(self.prefix),'--setup'],[],env=env);self.assertEqual(p.returncode,1,p.stdout);self.assertIn('macos_intel_unsupported',p.stdout);self.assertFalse(marker.exists());self.assertEqual(before,{p.relative_to(self.prefix):p.read_bytes() for p in self.prefix.rglob('*') if p.is_file()})
 def test_packaging_refuses_macos_intel_without_creating_output(self):
  out=self.root/'packages/tv-control-x86_64-apple-darwin.tar.gz'
  p=subprocess.run([sys.executable,str(ROOT/'scripts/package.py'),str(self.root/'unused'),str(out)],capture_output=True,text=True,timeout=5);self.assertNotEqual(p.returncode,0);self.assertIn('unsupported_package_target',p.stderr);self.assertFalse(out.parent.exists())
 def test_checksum_and_pin_fail_before_publication(self):
  archive=self.asset();p=self.install(archive,extra=['--sha256','0'*64]);self.assertEqual(p.value['code'],'pinned_checksum_mismatch');self.assertFalse(self.prefix.exists())
  archive.write_bytes(archive.read_bytes()+b'changed');p=self.install(archive);self.assertEqual(p.value['code'],'archive_checksum_mismatch');self.assertFalse(self.prefix.exists())
 def test_unsafe_archive_members_and_links_are_rejected(self):
  for mutation in ['traversal','absolute','symlink','hardlink','duplicate','extra']:
   p=self.install(self.asset(mutation=mutation));self.assertNotEqual(p.returncode,0,mutation);self.assertFalse(self.prefix.exists());self.assertFalse((self.root/'escape').exists())
 def test_existing_directory_and_symlink_are_not_clobbered(self):
  archive=self.asset();self.prefix.mkdir();keep=self.prefix/'keep';keep.write_text('keep');p=self.install(archive);self.assertEqual(p.value['code'],'prefix_not_owned_installation');self.assertEqual(keep.read_text(),'keep');keep.unlink();self.prefix.rmdir()
  target=self.root/'target';target.mkdir();self.prefix.symlink_to(target,target_is_directory=True);p=self.install(archive);self.assertEqual(p.value['code'],'symlink_path_refused');self.assertTrue(self.prefix.is_symlink())
 def test_modified_or_extra_files_block_uninstall_and_update(self):
  archive=self.asset();self.assertEqual(self.install(archive).returncode,0);binary=self.prefix/'versions/v0.3.0/tv-control';binary.write_text('changed')
  p=self.run_script(['--prefix',str(self.prefix),'--uninstall','--json']);self.assertEqual(p.value['code'],'installed_file_modified');self.assertTrue(binary.exists())
 def test_extra_files_and_failed_update_preserve_active_installation(self):
  old=self.asset('v0.2.0');self.assertEqual(self.install(old,'v0.2.0').returncode,0)
  new=self.asset();new.write_bytes(new.read_bytes()+b'changed');p=self.install(new,extra=['--update']);self.assertEqual(p.value['code'],'archive_checksum_mismatch');self.assertEqual((self.prefix/'current').read_text(),'v0.2.0\n')
  unknown=self.prefix/'user-data';unknown.write_text('keep');p=self.run_script(['--prefix',str(self.prefix),'--uninstall','--json']);self.assertEqual(p.value['code'],'unexpected_installation_files');self.assertEqual(unknown.read_text(),'keep');self.assertTrue((self.prefix/'tv-control').exists())
 def test_interruption_cleans_stage_and_lock_without_publication(self):
  import signal,time
  ready=self.root/'ready';curl=self.bin/'curl';curl.write_text('#!/bin/sh\nprintf ready > "${TV_TEST_READY:?}"\nsleep 30\n');curl.chmod(0o700)
  env=self.env.copy();env['TV_TEST_READY']=str(ready)
  proc=subprocess.Popen(['/bin/sh',str(ROOT/'install.sh'),'--prefix',str(self.prefix),'--json'],stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env,start_new_session=True)
  try:
   deadline=time.monotonic()+5
   while not ready.exists() and time.monotonic()<deadline:time.sleep(.01)
   self.assertTrue(ready.exists());os.killpg(proc.pid,signal.SIGTERM);proc.communicate(timeout=3)
   self.assertFalse(self.prefix.exists());self.assertFalse(list(self.root.glob('.tv-control-stage.*')));self.assertFalse(list(self.root.glob('.*.install-lock')))
  finally:
   if proc.poll() is None:os.killpg(proc.pid,signal.SIGKILL);proc.communicate(timeout=3)
 def test_special_entry_rejected_before_any_uninstall_change(self):
  archive=self.asset();self.assertEqual(self.install(archive).returncode,0)
  before={str(p.relative_to(self.prefix)):p.read_bytes() for p in self.prefix.rglob('*') if p.is_file()}
  fifo=self.prefix/'unrelated-fifo';os.mkfifo(fifo)
  p=self.run_script(['--prefix',str(self.prefix),'--uninstall','--json']);self.assertEqual(p.value['code'],'unexpected_installation_entry')
  after={str(p.relative_to(self.prefix)):p.read_bytes() for p in self.prefix.rglob('*') if p.is_file()};self.assertEqual(after,before);self.assertTrue(fifo.exists())
 def test_wrong_binary_machine_preserves_current_version(self):
  target='x86_64-linux-musl';old=self.asset('v0.2.0',target);env=self.env.copy();env.update(TV_TEST_OS='Linux',TV_TEST_ARCH='x86_64')
  args=['--prefix',str(self.prefix),'--version','v0.2.0','--archive',str(old),'--json'];self.assertEqual(self.run_script(args,env=env).returncode,0)
  wrong=self.asset('v0.3.0',target,binary_target='aarch64-linux-musl');p=self.run_script(['--prefix',str(self.prefix),'--version','v0.3.0','--archive',str(wrong),'--update','--json'],env=env)
  self.assertEqual(p.value['code'],'binary_architecture_mismatch');self.assertEqual((self.prefix/'current').read_text(),'v0.2.0\n');self.assertFalse((self.prefix/'versions/v0.3.0').exists())
 def test_dry_run_and_agent_mode_never_start_setup(self):
  archive=self.asset();marker=self.root/'setup-marker';self.env['TV_TEST_SETUP_MARKER']=str(marker)
  p=self.install(archive,extra=['--dry-run']);self.assertEqual(p.value['status'],'planned');self.assertFalse(self.prefix.exists());self.assertFalse(marker.exists())
  p=self.install(archive);self.assertEqual(p.returncode,0,p.stdout+p.stderr);self.assertFalse(p.value['setup_started']);self.assertFalse(marker.exists())
 def test_interactive_confirmation_cancel_and_explicit_setup(self):
  archive=self.asset();args=['--prefix',str(self.prefix),'--archive',str(archive),'--interactive']
  p=self.run_terminal(args,[('Continue? Type yes:',b'no\n')]);self.assertEqual(p.returncode,2,p.stdout);self.assertFalse(self.prefix.exists());self.assertIn('Cancelled; no changes made.',p.stdout);self.assertNotIn('Installation directory:',p.stdout)
  p=self.run_terminal(args,[('Continue? Type yes:',b'\x04')]);self.assertEqual(p.returncode,2,p.stdout);self.assertFalse(self.prefix.exists())
  marker=self.root/'setup-marker';env=self.env.copy();env['TV_TEST_SETUP_MARKER']=str(marker);env['XDG_CONFIG_HOME']=str(self.root/'config')
  p=self.run_terminal(args,[('Continue? Type yes:',b'yes\n'),('[y/N]:',b'y\n')],env=env);self.assertEqual(p.returncode,0,p.stdout);self.assertEqual(marker.read_text(),'setup\n')
 def test_explicit_setup_flag_is_interactive_only(self):
  archive=self.asset();marker=self.root/'explicit-setup';env=self.env.copy();env['TV_TEST_SETUP_MARKER']=str(marker);env['XDG_CONFIG_HOME']=str(self.root/'config')
  p=self.run_script(['--prefix',str(self.prefix),'--archive',str(archive),'--setup','--json'],env=env);self.assertEqual(p.value['code'],'conflicting_interaction_modes');self.assertFalse(self.prefix.exists())
  p=self.run_terminal(['--prefix',str(self.prefix),'--archive',str(archive),'--setup'],[('Continue? Type yes:',b'yes\n')],env=env);self.assertEqual(p.returncode,0,p.stdout);self.assertEqual(marker.read_text(),'setup\n')
 def test_interactive_without_controlling_terminal_fails_before_writes(self):
  archive=self.asset()
  for shell in SHELLS:
   for option in ['--interactive','--setup']:
    p=self.run_script(['--prefix',str(self.prefix),'--archive',str(archive),option],sh=shell,input='yes\ny\n')
    self.assertEqual(p.returncode,1,p.stdout+p.stderr);self.assertIn('interactive_terminal_required',p.stderr);self.assertNotIn('Continue?',p.stderr);self.assertFalse(self.prefix.exists());self.assertFalse(list(self.root.glob('.tv-control-stage.*')))
   p=self.run_script(['--prefix',str(self.prefix),'--setup','--dry-run'],sh=shell);self.assertEqual(p.returncode,0,p.stdout+p.stderr);self.assertFalse(self.prefix.exists())
 def test_fragmented_script_pipe_and_wizard_use_controlling_terminal_all_shells(self):
  archive=self.asset();marker=self.root/'piped-setup';env=self.env.copy();env.update(TV_TEST_SETUP_MARKER=str(marker),TV_TEST_SETUP_INPUT='yes',XDG_CONFIG_HOME=str(self.root/'config'))
  for shell in SHELLS:
   with self.subTest(shell=shell):
    p=self.run_terminal(['--prefix',str(self.prefix),'--archive',str(archive),'--setup'],[('Continue? Type yes:',b'yes\n'),('Wizard answer:',b'wizard-answer\n')],env=env,sh=shell,fragmented=True)
    self.assertEqual(p.returncode,0,p.stdout);self.assertEqual(marker.read_text(),'setup\n');self.assertNotIn('Cancelled',p.stdout)
    self.assertEqual(self.run_script(['--prefix',str(self.prefix),'--uninstall','--json'],sh=shell).returncode,0);marker.unlink()
 def test_fragmented_pipe_cancellation_does_not_install(self):
  archive=self.asset()
  for shell in SHELLS:
   p=self.run_terminal(['--prefix',str(self.prefix),'--archive',str(archive),'--setup'],[('Continue? Type yes:',b'no\n')],sh=shell,fragmented=True)
   self.assertEqual(p.returncode,2,p.stdout);self.assertIn('Cancelled; no changes made.',p.stdout);self.assertNotIn('Installation directory:',p.stdout);self.assertFalse(self.prefix.exists())
 def test_piped_agent_without_terminal_has_one_json_result_no_prompts(self):
  archive=self.asset()
  for shell in SHELLS:
   p=subprocess.run([*shell,'-s','--','--prefix',str(self.prefix),'--archive',str(archive),'--json'],input=(ROOT/'install.sh').read_text(),capture_output=True,text=True,env=self.env,timeout=15,start_new_session=True)
   self.assertEqual(p.returncode,0,p.stdout+p.stderr);self.assertEqual(len(p.stdout.splitlines()),1);self.assertEqual(json.loads(p.stdout)['status'],'installed');self.assertNotIn('Continue?',p.stderr)
   self.assertEqual(self.run_script(['--prefix',str(self.prefix),'--uninstall','--json'],sh=shell).returncode,0)
 def test_truncated_script_pipe_does_not_execute_installation_prefix(self):
  archive=self.asset();data=(ROOT/'install.sh').read_text().split('os=$(uname -s)',1)[0]
  for shell in SHELLS:
   p=subprocess.run([*shell,'-s','--','--prefix',str(self.prefix),'--archive',str(archive),'--non-interactive'],input=data,capture_output=True,text=True,env=self.env,timeout=5,start_new_session=True)
   self.assertNotEqual(p.returncode,0,p.stdout+p.stderr);self.assertFalse(self.prefix.exists());self.assertFalse(list(self.root.glob('.tv-control-stage.*')));self.assertFalse(list(self.root.glob('.*install-lock')))
 def zero_argument_environment(self):
  home=self.root/'home';home.mkdir(mode=0o700);self.prefix=home/'.local/share/tv-control';archive=self.asset();log=self.root/'downloads';marker=self.root/'setup-marker'
  curl=self.bin/'curl';curl.write_text('''#!/bin/sh
printf 'download\\n' >> "$TV_TEST_CURL_LOG"
output=
url=
while [ "$#" -gt 0 ]; do
 case "$1" in --output) output=$2; shift;; *) url=$1;; esac
 shift
done
case "$url" in *.sha256) cp "$TV_TEST_CHECKSUM" "$output";; *) cp "$TV_TEST_ARCHIVE" "$output";; esac
''');curl.chmod(0o700)
  env=self.env.copy();env.update(HOME=str(home),XDG_CONFIG_HOME=str(home/'.config'),TV_TEST_CURL_LOG=str(log),TV_TEST_ARCHIVE=str(archive),TV_TEST_CHECKSUM=str(archive)+'.sha256',TV_TEST_SETUP_MARKER=str(marker))
  return env,log,marker
 def test_zero_arguments_install_and_rerun_resume_without_download(self):
  env,log,marker=self.zero_argument_environment()
  for count in range(2):
   p=self.run_terminal([],[('Continue? Type yes:',b'yes\n')],env=env,fragmented=True);self.assertEqual(p.returncode,0,p.stdout);self.assertEqual(marker.read_text(),'setup\n');self.assertEqual(log.read_text().splitlines(),['download','download']);self.assertEqual((self.prefix/'current').read_text(),'v0.3.0\n')
   self.assertFalse(list(self.prefix.parent.glob('.*.install-lock')));self.assertFalse(list(self.prefix.parent.glob('.tv-control-stage.*')))
 def test_zero_arguments_resume_after_setup_failure_and_update_old_version(self):
  env,log,marker=self.zero_argument_environment();old=self.asset('v0.2.0');self.assertEqual(self.install(old,'v0.2.0').returncode,0)
  env['TV_TEST_SETUP_FAIL_ONCE']=str(self.root/'failure-once')
  p=self.run_terminal([],[('Continue? Type yes:',b'yes\n')],env=env,fragmented=True);self.assertEqual(p.returncode,31,p.stdout);self.assertFalse(marker.exists());self.assertEqual((self.prefix/'current').read_text(),'v0.3.0\n');self.assertTrue((self.prefix/'versions/v0.2.0').exists());self.assertFalse(list(self.prefix.parent.glob('.*.install-lock')))
  p=self.run_terminal([],[('Continue? Type yes:',b'yes\n')],env=env,fragmented=True);self.assertEqual(p.returncode,0,p.stdout);self.assertEqual(marker.read_text(),'setup\n');self.assertEqual(len(log.read_text().splitlines()),2)
 def test_zero_arguments_cancel_and_missing_terminal_do_not_write(self):
  env,log,marker=self.zero_argument_environment()
  p=self.run_terminal([],[('Continue? Type yes:',b'no\n')],env=env,fragmented=True);self.assertEqual(p.returncode,2,p.stdout);self.assertFalse(self.prefix.exists());self.assertFalse(log.exists());self.assertFalse(marker.exists())
  p=self.run_script([],env=env,input='yes\n');self.assertEqual(p.returncode,1,p.stdout+p.stderr);self.assertIn('interactive_terminal_required',p.stderr);self.assertFalse(self.prefix.exists());self.assertFalse(log.exists())
 def test_existing_setup_reuse_checks_pin_and_refuses_archive_override(self):
  archive=self.asset();self.assertEqual(self.install(archive).returncode,0);marker=self.root/'setup-marker';env=self.env.copy();env.update(TV_TEST_SETUP_MARKER=str(marker),XDG_CONFIG_HOME=str(self.root/'config'))
  args=['--prefix',str(self.prefix),'--version','v0.3.0','--setup']
  p=self.run_terminal([*args,'--sha256','0'*64],[('Continue? Type yes:',b'yes\n')],env=env);self.assertEqual(p.returncode,1,p.stdout);self.assertIn('checksum_mismatch',p.stdout);self.assertFalse(marker.exists())
  p=self.run_terminal([*args,'--archive',str(archive)],[('Continue? Type yes:',b'yes\n')],env=env);self.assertEqual(p.returncode,1,p.stdout);self.assertIn('setup_reuse_refuses_archive_override',p.stdout);self.assertFalse(marker.exists())
  digest=hashlib.sha256(archive.read_bytes()).hexdigest();p=self.run_terminal([*args,'--sha256',digest],[('Continue? Type yes:',b'yes\n')],env=env);self.assertEqual(p.returncode,0,p.stdout);self.assertTrue(marker.exists())
 def test_https_request_plan_uses_pinned_assets_and_flags(self):
  archive=self.asset();log=self.root/'curl-args';curl=self.bin/'curl'
  curl.write_text('''#!/bin/sh
printf '%s\\n' "$@" >> "$TV_TEST_CURL_LOG"
output=
url=
while [ "$#" -gt 0 ]; do
 case "$1" in --output) output=$2; shift;; *) url=$1;; esac
 shift
done
case "$url" in *.sha256) cp "$TV_TEST_CHECKSUM" "$output";; *) cp "$TV_TEST_ARCHIVE" "$output";; esac
''');curl.chmod(0o700)
  env=self.env.copy();env.update(TV_TEST_CURL_LOG=str(log),TV_TEST_ARCHIVE=str(archive),TV_TEST_CHECKSUM=str(archive)+'.sha256')
  p=self.run_script(['--prefix',str(self.prefix),'--version','v0.3.0','--json'],env=env);self.assertEqual(p.returncode,0,p.stdout+p.stderr)
  text=log.read_text();self.assertIn('--proto',text);self.assertIn('--proto-redir',text);self.assertEqual(text.count('https://github.com/antoine-berger/tv-control/releases/download/v0.3.0/'),2);self.assertNotIn('latest',text)
 def test_download_failure_cleans_temporary_work(self):
  curl=self.bin/'curl';curl.write_text('#!/bin/sh\nexit 22\n');curl.chmod(0o700)
  p=self.run_script(['--prefix',str(self.prefix),'--json']);self.assertEqual(p.value['code'],'https_download_failed');self.assertFalse(self.prefix.exists());self.assertFalse(list(self.root.glob('.tv-control-stage.*')))
 def test_io_failures_emit_one_json_error_and_preserve_active_version(self):
  old=self.asset('v0.2.0');self.assertEqual(self.install(old,'v0.2.0').returncode,0);new=self.asset()
  for command in ['cp','chmod']:
   mock=self.bin/command;mock.write_text('#!/bin/sh\nexit 73\n');mock.chmod(0o700)
   p=self.install(new,extra=['--update']);self.assertEqual(p.returncode,73,p.stdout+p.stderr);self.assertEqual(p.value['code'],'installer_operation_failed');self.assertEqual((self.prefix/'current').read_text(),'v0.2.0\n');self.assertFalse((self.prefix/'versions/v0.3.0').exists());self.assertFalse(list(self.root.glob('.tv-control-stage.*')))
   mock.unlink()
 def test_invalid_version_and_unknown_option_do_not_write(self):
  p=self.run_script(['--prefix',str(self.prefix),'--version','v0.3.0\n../escape','--json']);self.assertEqual(p.value['code'],'invalid_version');self.assertFalse(self.prefix.exists())
  p=self.run_script(['--prefix',str(self.prefix),'--bad','--json']);self.assertNotEqual(p.returncode,0);self.assertFalse(self.prefix.exists())
if __name__=='__main__':
 print('Installer shells: '+', '.join(' '.join(s) for s in SHELLS),file=sys.stderr)
 unittest.main()
