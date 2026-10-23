#!/usr/bin/env python3
"""Build an explicit allowlist archive, without private config or build caches."""
import hashlib
import json
import pathlib
import sys
import tarfile

root = pathlib.Path(__file__).resolve().parent.parent
binary = pathlib.Path(sys.argv[1])
output = pathlib.Path(sys.argv[2])
supported = {"tv-control-aarch64-apple-darwin.tar.gz", "tv-control-aarch64-linux-musl.tar.gz", "tv-control-x86_64-linux-musl.tar.gz"}
if output.name not in supported:
    raise SystemExit("unsupported_package_target")
output.parent.mkdir(parents=True, exist_ok=True)
files = [(binary, "tv-control"), (root / "LICENSE", "LICENSE"),
         (root / "README.md", "README.md"),
         (root / "install.sh", "install.sh"),
         (root / "docs/INSTALLATION.md", "docs/INSTALLATION.md"),
         (root / "examples/config.json", "examples/config.json"),
         (root / "examples/mcp.json", "examples/mcp.json"),
         (root / "examples/discovery.json", "examples/discovery.json"),
         (root / "docs/ONBOARDING.md", "docs/ONBOARDING.md")]
for name in ("ACCEPTANCE", "ADAPTERS", "ADR", "CLI", "CONFIGURATION", "DEVELOPMENT", "MCP", "REVIEW", "ROUTER"):
    files.append((root / "docs" / f"{name}.md", f"docs/{name}.md"))
files.extend([(root / "PLAN.md", "PLAN.md"), (root / "benchmarks/README.md", "benchmarks/README.md"),
              (root / "benchmarks/host-macos-arm64.json", "benchmarks/host-macos-arm64.json")])
with tarfile.open(output, "w:gz") as archive:
    for path, name in files:
        info = archive.gettarinfo(str(path), name)
        info.uid = info.gid = 0
        info.uname = info.gname = ""
        info.mtime = 0
        with path.open("rb") as source:
            archive.addfile(info, source)
digest = hashlib.sha256(output.read_bytes()).hexdigest()
output.with_suffix(output.suffix + ".sha256").write_text(f"{digest}  {output.name}\n")
print(json.dumps({"archive": output.name, "sha256": digest, "members": [name for _, name in files]}))

installer = output.parent / "install.sh"
installer.write_bytes((root / "install.sh").read_bytes())
installer.with_suffix(".sh.sha256").write_text(f"{hashlib.sha256(installer.read_bytes()).hexdigest()}  install.sh\n")
