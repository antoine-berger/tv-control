#!/usr/bin/env python3
"""Installer contracts: synthetic local assets and temporary prefixes only, never HOME installs."""
import hashlib,io,json,os,pathlib,shutil,subprocess,sys,tarfile,tempfile,unittest,platform,struct
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
int main(int argc,char **argv) {
 if(argc>1 && strcmp(argv[1],"--version")==0) {printf("fixture:%s\n",argv[0]);return 0;}
 if(argc>1 && strcmp(argv[1],"setup")==0) {const char *p=getenv("TV_TEST_SETUP_MARKER");if(!p)return 7;FILE*f=fopen(p,"w");if(!f)return 8;fputs("setup\n",f);return fclose(f);}
 return 0;
}
''');binary=root/'stub';subprocess.run(['cc',str(source),'-o',str(binary)],check=True,capture_output=True);cls.native_stub=binary.read_bytes()
 @classmethod
 def tearDownClass(cls):cls.compiler_tmp.cleanup()
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory(prefix="installer test's ");self.root=pathlib.Path(self.tmp.name).resolve();self.prefix=self.root/"app's files"
  self.bin=self.root/'commands';self.bin.mkdir();uname=self.bin/'uname';uname.write_text('#!/bin/sh\ncase "$1" in -s) printf "%s\\n" "$TV_TEST_OS";; -m) printf "%s\\n" "$TV_TEST_ARCH";; esac\n');uname.chmod(0o700)
  self.env=os.environ.copy();self.env.update(PATH=str(self.bin)+os.pathsep+os.environ['PATH'],TV_TEST_OS=platform.system(),TV_TEST_ARCH=platform.machine())
 def tearDown(self):self.tmp.cleanup()
 def asset(self,version='v0.2.1',target=HOST_TARGET,mutation=None,binary_target=None):
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
  p=subprocess.run([*(sh or SHELLS[0]),str(ROOT/'install.sh'),*args],input=input,text=True,capture_output=True,env=env or self.env,timeout=15)
  if '--json' in args:
   self.assertEqual(len(p.stdout.splitlines()),1,p.stdout+p.stderr);p.value=json.loads(p.stdout)
  return p
 def install(self,archive,version='v0.2.1',extra=(),sh=None):return self.run_script(['--prefix',str(self.prefix),'--version',version,'--archive',str(archive),'--json',*extra],sh=sh)
 def test_all_shells_install_launch_update_and_uninstall_without_user_config_removal(self):
  old=self.asset('v0.2.0');new=self.asset()
  for sh in SHELLS:
   with self.subTest(shell=sh):
    p=self.install(old,'v0.2.0',sh=sh);self.assertEqual(p.returncode,0,p.stdout+p.stderr);self.assertEqual(p.value['status'],'installed');self.assertEqual(self.prefix.stat().st_mode&0o777,0o700)
    p=subprocess.run([str(self.prefix/'tv-control'),'--version'],text=True,capture_output=True,env=self.env);self.assertIn('/versions/v0.2.0/tv-control',p.stdout)
    p=self.install(new,extra=['--update'],sh=sh);self.assertEqual(p.returncode,0,p.stdout+p.stderr);self.assertEqual((self.prefix/'current').read_text(),'v0.2.1\n');self.assertTrue((self.prefix/'versions/v0.2.0').is_dir())
    marker=self.root/'user-config.local.json';marker.write_text('keep')
    p=self.run_script(['--prefix',str(self.prefix),'--uninstall','--json'],sh=sh);self.assertEqual(p.returncode,0,p.stdout+p.stderr);self.assertFalse(self.prefix.exists());self.assertEqual(marker.read_text(),'keep')
    self.assertFalse(list(self.root.glob('.tv-control-stage.*')));self.assertFalse(list(self.root.glob('.*.install-lock')))
 def test_platform_asset_mapping_for_all_supported_hosts(self):
  for system,machine,target in [('Darwin','arm64','aarch64-apple-darwin'),('Darwin','x86_64','x86_64-apple-darwin'),('Linux','aarch64','aarch64-linux-musl'),('Linux','x86_64','x86_64-linux-musl')]:
   env=self.env.copy();env.update(TV_TEST_OS=system,TV_TEST_ARCH=machine);archive=self.asset(target=target)
   p=self.run_script(['--prefix',str(self.prefix),'--archive',str(archive),'--json'],env=env);self.assertEqual(p.returncode,0,p.stdout+p.stderr)
   p=self.run_script(['--prefix',str(self.prefix),'--uninstall','--json'],env=env);self.assertEqual(p.returncode,0,p.stdout+p.stderr)
 def test_checksum_and_pin_fail_before_publication(self):
  archive=self.asset();p=self.install(archive,extra=['--sha256','0'*64]);self.assertEqual(p.value['code'],'pinned_checksum_mismatch');self.assertFalse(self.prefix.exists())
  archive.write_bytes(archive.read_bytes()+b'changed');p=self.install(archive);self.assertEqual(p.value['code'],'archive_checksum_mismatch');self.assertFalse(self.prefix.exists())
 def test_unsafe_archive_members_and_links_are_rejected(self):
  for mutation in ['traversal','absolute','symlink','hardlink','duplicate','extra']:
   p=self.install(self.asset(mutation=mutation));self.assertNotEqual(p.returncode,0,mutation);self.assertFalse(self.prefix.exists());self.assertFalse((self.root/'escape').exists())
 def test_existing_directory_and_symlink_are_not_clobbered(self):
  archive=self.asset();self.prefix.mkdir();keep=self.prefix/'keep';keep.write_text('keep');p=self.install(archive);self.assertEqual(p.value['code'],'installation_exists_use_update');self.assertEqual(keep.read_text(),'keep');keep.unlink();self.prefix.rmdir()
  target=self.root/'target';target.mkdir();self.prefix.symlink_to(target,target_is_directory=True);p=self.install(archive);self.assertEqual(p.value['code'],'symlink_path_refused');self.assertTrue(self.prefix.is_symlink())
 def test_modified_or_extra_files_block_uninstall_and_update(self):
  archive=self.asset();self.assertEqual(self.install(archive).returncode,0);binary=self.prefix/'versions/v0.2.1/tv-control';binary.write_text('changed')
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
  wrong=self.asset('v0.2.1',target,binary_target='aarch64-linux-musl');p=self.run_script(['--prefix',str(self.prefix),'--version','v0.2.1','--archive',str(wrong),'--update','--json'],env=env)
  self.assertEqual(p.value['code'],'binary_architecture_mismatch');self.assertEqual((self.prefix/'current').read_text(),'v0.2.0\n');self.assertFalse((self.prefix/'versions/v0.2.1').exists())
 def test_dry_run_and_agent_mode_never_start_setup(self):
  archive=self.asset();marker=self.root/'setup-marker';self.env['TV_TEST_SETUP_MARKER']=str(marker)
  p=self.install(archive,extra=['--dry-run']);self.assertEqual(p.value['status'],'planned');self.assertFalse(self.prefix.exists());self.assertFalse(marker.exists())
  p=self.install(archive);self.assertEqual(p.returncode,0,p.stdout+p.stderr);self.assertFalse(p.value['setup_started']);self.assertFalse(marker.exists())
 def test_interactive_confirmation_cancel_and_explicit_setup(self):
  archive=self.asset();args=['--prefix',str(self.prefix),'--archive',str(archive),'--interactive']
  p=self.run_script(args,input='no\n');self.assertEqual(p.returncode,2,p.stderr);self.assertFalse(self.prefix.exists())
  p=self.run_script(args,input='');self.assertEqual(p.returncode,1);self.assertFalse(self.prefix.exists())
  marker=self.root/'setup-marker';env=self.env.copy();env['TV_TEST_SETUP_MARKER']=str(marker);env['XDG_CONFIG_HOME']=str(self.root/'config')
  p=self.run_script(args,input='yes\ny\n',env=env);self.assertEqual(p.returncode,0,p.stdout+p.stderr);self.assertEqual(marker.read_text(),'setup\n')
 def test_explicit_setup_flag_is_interactive_only(self):
  archive=self.asset();marker=self.root/'explicit-setup';env=self.env.copy();env['TV_TEST_SETUP_MARKER']=str(marker);env['XDG_CONFIG_HOME']=str(self.root/'config')
  p=self.run_script(['--prefix',str(self.prefix),'--archive',str(archive),'--setup','--json'],env=env);self.assertEqual(p.value['code'],'conflicting_interaction_modes');self.assertFalse(self.prefix.exists())
  p=self.run_script(['--prefix',str(self.prefix),'--archive',str(archive),'--setup'],input='yes\n',env=env);self.assertEqual(p.returncode,0,p.stdout+p.stderr);self.assertEqual(marker.read_text(),'setup\n')
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
  p=self.run_script(['--prefix',str(self.prefix),'--version','v0.2.1','--json'],env=env);self.assertEqual(p.returncode,0,p.stdout+p.stderr)
  text=log.read_text();self.assertIn('--proto',text);self.assertIn('--proto-redir',text);self.assertEqual(text.count('https://github.com/antoine-berger/tv-control/releases/download/v0.2.1/'),2);self.assertNotIn('latest',text)
 def test_download_failure_cleans_temporary_work(self):
  curl=self.bin/'curl';curl.write_text('#!/bin/sh\nexit 22\n');curl.chmod(0o700)
  p=self.run_script(['--prefix',str(self.prefix),'--json']);self.assertEqual(p.value['code'],'https_download_failed');self.assertFalse(self.prefix.exists());self.assertFalse(list(self.root.glob('.tv-control-stage.*')))
 def test_io_failures_emit_one_json_error_and_preserve_active_version(self):
  old=self.asset('v0.2.0');self.assertEqual(self.install(old,'v0.2.0').returncode,0);new=self.asset()
  for command in ['cp','chmod']:
   mock=self.bin/command;mock.write_text('#!/bin/sh\nexit 73\n');mock.chmod(0o700)
   p=self.install(new,extra=['--update']);self.assertEqual(p.returncode,73,p.stdout+p.stderr);self.assertEqual(p.value['code'],'installer_operation_failed');self.assertEqual((self.prefix/'current').read_text(),'v0.2.0\n');self.assertFalse((self.prefix/'versions/v0.2.1').exists());self.assertFalse(list(self.root.glob('.tv-control-stage.*')))
   mock.unlink()
 def test_invalid_version_and_unknown_option_do_not_write(self):
  p=self.run_script(['--prefix',str(self.prefix),'--version','v0.2.1\n../escape','--json']);self.assertEqual(p.value['code'],'invalid_version');self.assertFalse(self.prefix.exists())
  p=self.run_script(['--prefix',str(self.prefix),'--bad','--json']);self.assertNotEqual(p.returncode,0);self.assertFalse(self.prefix.exists())
if __name__=='__main__':
 print('Installer shells: '+', '.join(' '.join(s) for s in SHELLS),file=sys.stderr)
 unittest.main()
