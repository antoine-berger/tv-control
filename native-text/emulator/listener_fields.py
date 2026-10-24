"""Strict parser for Darwin lsof -nP -a -p PID -iTCP:PORT -sTCP:LISTEN -Fpn."""
import re


def owned_loopback_listener(output, pid, port):
    # lsof -F? documents that f (file descriptor) is always emitted, even
    # when only p/n were requested. Every file record must still name an
    # endpoint, under the exact single process requested by the caller.
    lines = output.splitlines()
    if not lines or lines[0] != f"p{pid}" or len(lines) < 3 or len(lines) % 2 != 1:
        return False
    allowed = {f"n127.0.0.1:{port}", f"n[::1]:{port}"}
    descriptors = set()
    for index in range(1, len(lines), 2):
        descriptor, endpoint = lines[index:index + 2]
        if not re.fullmatch(r"f(?:0|[1-9][0-9]*)", descriptor):
            return False
        if descriptor in descriptors or endpoint not in allowed:
            return False
        descriptors.add(descriptor)
    return True
