"""Owned local pipes only. Killing/reaping these never proves remote cleanup."""
import os
import select
import time
from process_group import leader_exited, require_nonreaping_wait, retained_exit_info, stop_unreaped_group
from transport_wire import TransportFault


class PipedChild:
    """Adopt a registered direct child started in its own session by the guarded launcher.

    The launcher must register ownership before configuring pipes and retain that
    registry even if setup raises. No argv construction, SDK discovery or daemon
    startup occurs here. stdin/stdout/stderr must be private binary pipes.
    """
    @staticmethod
    def require_supported():
        require_nonreaping_wait()

    def __init__(self, process, register):
        self.process = process
        self._pending = bytearray()
        self._eof = {'stdout': False, 'stderr': False}
        self._input_closed = False
        self._cleanup_attempted = False
        self._stderr_bytes = 0
        self._group_owned = False
        register(self)  # Ownership exists before capability/FD setup can raise.
        require_nonreaping_wait()
        if os.getpgid(process.pid) != process.pid:
            raise TransportFault('HOST_FAILURE')
        self._group_owned = True
        if any(getattr(process, name) is None for name in ('stdin', 'stdout', 'stderr')):
            raise TransportFault('HOST_FAILURE')
        for name in ('stdin', 'stdout', 'stderr'):
            os.set_blocking(getattr(process, name).fileno(), False)

    def queue(self, data):
        if self._input_closed or not isinstance(data, bytes) or len(self._pending) + len(data) > 32768:
            raise TransportFault('LIMIT_EXCEEDED')
        self._pending.extend(data)

    def pending_bytes(self):
        return len(self._pending)

    def flush(self):
        if not self._pending or self._input_closed:
            return
        count = None
        try:
            count = os.write(self.process.stdin.fileno(), self._pending)
        except BlockingIOError:
            return
        except OSError:
            pass
        if count is None or count <= 0:
            raise TransportFault('CHANNEL_LOST')
        del self._pending[:count]

    def read(self, name):
        if name not in self._eof:
            raise TransportFault('HOST_FAILURE')
        if self._eof[name]:
            return None
        chunk = None
        failed = False
        try:
            chunk = os.read(getattr(self.process, name).fileno(), 4096)
        except BlockingIOError:
            return None
        except OSError:
            failed = True
        if failed:
            raise TransportFault('CHANNEL_LOST')
        if chunk == b'':
            self._eof[name] = True
        if name == 'stderr' and chunk:
            self._stderr_bytes += len(chunk)
            if self._stderr_bytes > 4096:
                raise TransportFault('LIMIT_EXCEEDED')
        return chunk

    def close_input(self):
        # Explicit cancellation may discard an incomplete COMMIT and signal EOF;
        # never append a CANCEL frame into a partially written COMMIT body.
        self._pending.clear()
        if not self._input_closed:
            self._input_closed = True
            self.process.stdin.close()

    def exited(self):
        return leader_exited(self.process)

    def exit_ok(self):
        return retained_exit_info(self.process) == {'state': 'exited', 'exit_code': 0}

    def wait_handles(self):
        reads = [getattr(self.process, name).fileno() for name, eof in self._eof.items() if not eof]
        writes = [] if self._input_closed or not self._pending else [self.process.stdin.fileno()]
        return reads, writes

    def cleanup(self):
        if self._cleanup_attempted:
            return False
        self._cleanup_attempted = True
        clean = False
        try:
            if self._group_owned:
                stop_unreaped_group(self.process, grace_seconds=0)
                clean = True
        except Exception:
            pass
        finally:
            for name in ('stdin', 'stdout', 'stderr'):
                try:
                    getattr(self.process, name).close()
                except Exception:
                    clean = False
            self._pending.clear()
        return clean


def wait_children(children, timeout):
    """Bounded readiness only; no reset-on-byte timeout or blocking communicate."""
    reads, writes = [], []
    for child in children:
        incoming, outgoing = child.wait_handles()
        reads.extend(incoming)
        writes.extend(outgoing)
    if reads or writes:
        select.select(reads, writes, [], max(0, min(timeout, .02)))
    else:
        time.sleep(max(0, min(timeout, .02)))
