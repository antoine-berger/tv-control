#!/usr/bin/env python3
"""Inspect a static Linux ELF without executing it. Reject INTERP and DT_NEEDED."""
import json
import pathlib
import struct
import sys

data = pathlib.Path(sys.argv[1]).read_bytes()
assert data[:6] == b"\x7fELF\x02\x01", "expected little-endian ELF64"
machine = {183: "aarch64", 62: "x86_64"}.get(struct.unpack_from("<H", data, 18)[0])
assert machine is not None, "unsupported ELF machine"
if len(sys.argv) > 2:
    assert machine == sys.argv[2], "ELF machine does not match requested target"
phoff = struct.unpack_from("<Q", data, 32)[0]
phsize, phnum = struct.unpack_from("<HH", data, 54)
assert phsize >= 56 and phoff + phsize * phnum <= len(data)
for i in range(phnum):
    kind, _, offset, _, _, size = struct.unpack_from("<IIQQQQ", data, phoff + i * phsize)
    assert kind != 3, "PT_INTERP present"
    if kind == 2:
        assert offset + size <= len(data)
        for pos in range(offset, offset + size, 16):
            tag, _ = struct.unpack_from("<QQ", data, pos)
            assert tag != 1, "DT_NEEDED present"
            if tag == 0:
                break
print(json.dumps({"machine": machine, "format": "ELF64", "PT_INTERP": False, "DT_NEEDED": False, "bytes": len(data)}))
