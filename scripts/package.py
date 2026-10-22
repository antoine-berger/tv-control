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
output.parent.mkdir(parents=True, exist_ok=True)
files = [(binary, "tv-control"), (root / "LICENSE", "LICENSE"),
         (root / "README.md", "README.md"),
         (root / "examples/config.json", "examples/config.json"),
         (root / "examples/mcp.json", "examples/mcp.json")]
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
