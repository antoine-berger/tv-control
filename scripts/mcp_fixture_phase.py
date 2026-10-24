"""Bounded HTTP-fixture synchronization for post-dispatch MCP uncertainty tests."""
from contextlib import contextmanager
import threading


class VerificationBarrier:
    """Hold the third post-POST snapshot, after two were consumed and rejected."""
    def __init__(self):
        self.posted = threading.Event()
        self.blocked = threading.Event()
        self.release = threading.Event()
        self.lock = threading.Lock()
        self.session_reads = 0
        self.post_count = 0

    def record_post(self):
        with self.lock:
            self.post_count += 1
        self.posted.set()

    def before_session_response(self):
        if not self.posted.is_set():
            return
        with self.lock:
            self.session_reads += 1
            block = self.session_reads == 3
        if block:
            self.blocked.set()
            self.release.wait(timeout=10)

    @contextmanager
    def cancel_on_exit(self, cancel):
        # Also run on a failed event wait/assertion: no HTTP handler or owned MCP
        # child is left waiting for the caller's otherwise comfortable deadline.
        try:
            yield
        finally:
            try:
                cancel()
            finally:
                self.release.set()
