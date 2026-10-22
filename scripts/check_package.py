#!/usr/bin/env python3
"""Verify checksums, exact archive allowlist, machine and optional offline bundle smoke test."""
import argparse,hashlib,json,pathlib,struct,subprocess,tarfile,tempfile
p=argparse.ArgumentParser();p.add_argument('archive',type=pathlib.Path);p.add_argument('--smoke',action='store_true');a=p.parse_args()
expected={'tv-control','LICENSE','README.md','PLAN.md','examples/config.json','examples/mcp.json','examples/discovery.json','benchmarks/README.md','benchmarks/host-macos-arm64.json'}
expected.update('docs/'+n+'.md' for n in ('ACCEPTANCE','ADAPTERS','ADR','CLI','CONFIGURATION','DEVELOPMENT','MCP','ONBOARDING','REVIEW','ROUTER'))
checksum=a.archive.with_suffix(a.archive.suffix+'.sha256').read_text().strip()
digest,filename=checksum.split('  ',1);assert filename==a.archive.name
assert hashlib.sha256(a.archive.read_bytes()).hexdigest()==digest,'checksum mismatch'
with tarfile.open(a.archive,'r:gz') as archive:
 members=archive.getmembers();assert len(members)==len(expected);assert {m.name for m in members}==expected
 assert all(m.isfile() and m.size<=10_000_000 and m.uid==m.gid==0 for m in members)
 content={m.name:archive.extractfile(m).read() for m in members}
binary=content['tv-control'];name=a.archive.name
if 'linux-musl' in name:
 assert binary[:6]==b'\x7fELF\x02\x01';machine=struct.unpack_from('<H',binary,18)[0]
 assert machine==({'aarch64':183,'x86_64':62}[name.split('-')[2]])
elif 'apple-darwin' in name:
 assert binary[:4]==b'\xcf\xfa\xed\xfe';machine=struct.unpack_from('<I',binary,4)[0]
 assert machine==({'aarch64':0x0100000c,'x86_64':0x01000007}[name.split('-')[2]])
else:raise AssertionError('unqualified package name')
json.loads(content['examples/config.json']);json.loads(content['examples/discovery.json'])
if a.smoke:
 with tempfile.TemporaryDirectory(prefix='tv-bundle-') as directory:
  root=pathlib.Path(directory)
  for relative,data in content.items():
   path=root/relative;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(data)
  executable=root/'tv-control';executable.chmod(0o700)
  result=subprocess.run([str(executable),'--version'],capture_output=True,text=True,timeout=5,check=True)
  assert result.stdout.strip()=='tv-control 0.2.0'
  result=subprocess.run([str(executable),'discover','--fixture',str(root/'examples/discovery.json'),'--interface','lan-fixture','--json'],capture_output=True,text=True,timeout=5,check=True)
  value=json.loads(result.stdout);assert value['simulation'] and len(value['candidates'])==3
print(json.dumps({'archive':a.archive.name,'sha256':digest,'members':len(expected),'offline_smoke':a.smoke}))
