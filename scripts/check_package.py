#!/usr/bin/env python3
"""Verify checksums, exact archive allowlist, machine and optional offline bundle smoke test."""
import argparse,hashlib,json,pathlib,struct,subprocess,tarfile,tempfile,re
p=argparse.ArgumentParser();p.add_argument('archive',type=pathlib.Path);p.add_argument('--smoke',action='store_true');a=p.parse_args()
expected={'tv-control','LICENSE','README.md','PLAN.md','examples/config.json','examples/mcp.json','examples/discovery.json','benchmarks/README.md','benchmarks/host-macos-arm64.json'}
expected.update({'install.sh','docs/INSTALLATION.md'})
expected.update('docs/'+n+'.md' for n in ('ACCEPTANCE','ADAPTERS','ADR','CLI','CONFIGURATION','DEVELOPMENT','MCP','ONBOARDING','REVIEW','ROUTER'))
checksum=a.archive.with_suffix(a.archive.suffix+'.sha256').read_text().strip()
digest,filename=checksum.split('  ',1);assert filename==a.archive.name
assert hashlib.sha256(a.archive.read_bytes()).hexdigest()==digest,'checksum mismatch'
with tarfile.open(a.archive,'r:gz') as archive:
 members=archive.getmembers();assert len(members)==len(expected);assert {m.name for m in members}==expected
 assert all(m.isfile() and m.size<=10_000_000 and m.uid==m.gid==0 for m in members)
 content={m.name:archive.extractfile(m).read() for m in members}
installer=a.archive.parent/'install.sh'
assert installer.read_bytes()==content['install.sh'], 'standalone installer differs from bundled source'
installer_sum=installer.with_suffix('.sh.sha256').read_text().strip()
assert installer_sum==hashlib.sha256(installer.read_bytes()).hexdigest()+'  install.sh'
binary=content['tv-control'];name=a.archive.name
if 'linux-musl' in name:
 assert binary[:6]==b'\x7fELF\x02\x01';machine=struct.unpack_from('<H',binary,18)[0]
 assert machine==({'aarch64':183,'x86_64':62}[name.split('-')[2]])
elif 'apple-darwin' in name:
 assert name=='tv-control-aarch64-apple-darwin.tar.gz','unsupported_package_target'
 assert binary[:4]==b'\xcf\xfa\xed\xfe';machine=struct.unpack_from('<I',binary,4)[0]
 assert machine==0x0100000c
else:raise AssertionError('unqualified package name')
json.loads(content['examples/config.json']);json.loads(content['examples/discovery.json'])
if a.smoke:
 with tempfile.TemporaryDirectory(prefix='tv-bundle-') as directory:
  root=pathlib.Path(directory)
  for relative,data in content.items():
   path=root/relative;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(data)
  executable=root/'tv-control';executable.chmod(0o700)
  result=subprocess.run([str(executable),'--version'],capture_output=True,text=True,timeout=5,check=True)
  version=re.search(r'^version = "([^"]+)"', (pathlib.Path(__file__).resolve().parent.parent/'Cargo.toml').read_text(),re.M).group(1)
  assert result.stdout.strip()=='tv-control '+version
  result=subprocess.run([str(executable),'discover','--fixture',str(root/'examples/discovery.json'),'--interface','lan-fixture','--json'],capture_output=True,text=True,timeout=5,check=True)
  value=json.loads(result.stdout);assert value['simulation'] and len(value['candidates'])==3
  # The shipped installer is exercised only under a disposable canonical prefix.
  installed=root.resolve()/"installed app's files"
  installer=root.resolve()/'install.sh'
  subprocess.run(['sh',str(installer),'--non-interactive','--version','v'+version,'--archive',str(a.archive.resolve()),'--prefix',str(installed),'--json'],capture_output=True,text=True,timeout=30,check=True)
  result=subprocess.run([str(installed/'tv-control'),'--version'],capture_output=True,text=True,timeout=5,check=True)
  assert result.stdout.strip()=='tv-control '+version
  result=subprocess.run([str(installed/'tv-control'),'discover','--fixture',str(installed/'versions'/('v'+version)/'examples/discovery.json'),'--interface','lan-fixture','--json'],capture_output=True,text=True,timeout=5,check=True)
  assert json.loads(result.stdout)['simulation']
  subprocess.run(['sh',str(installer),'--non-interactive','--uninstall','--prefix',str(installed),'--json'],capture_output=True,text=True,timeout=30,check=True)
  assert not installed.exists()

print(json.dumps({'archive':a.archive.name,'sha256':digest,'members':len(expected),'offline_smoke':a.smoke}))
