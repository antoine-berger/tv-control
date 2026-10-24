"""Lab-only two-channel supervision. Not a production capability or free-serial CLI."""
import secrets
import time
from transport_bundle import CLEANUP, REASON, TEXT, Instrumentation
from transport_pipes import wait_children
from transport_wire import InputFrames, PrivateAppend, Responses, TransportFault


class Outcome:
    def __init__(self, status, reason, server_cleanup, client_cleanup, local_cleanup, qualified):
        self.status, self.reason = status, reason
        self.server_cleanup, self.client_cleanup = server_cleanup, client_cleanup
        self.local_cleanup, self.qualified = local_cleanup, qualified

    def public(self):
        return {'status': self.status, 'reason': self.reason,
                'server_cleanup': self.server_cleanup, 'client_cleanup': self.client_cleanup,
                'local_cleanup': self.local_cleanup, 'qualified': self.qualified,
                'cleanup': 'VERIFIED' if self.qualified else 'UNVERIFIED'}


class Host:
    """The injected launcher belongs to the existing, exclusively owned AVD runtime.

    guard() must revalidate the private ADB owner and pinned helper/user/UID/artifact
    identity. instrumentation(instance) and client(instance) launch only the reviewed
    fixed commands, registering each owned child before any operation can raise.
    owned() must retain every such channel, including a partially failed launch.
    It must never restart/connect another ADB server or accept an arbitrary serial.
    No real launcher or command-line adapter is selected by this module.
    """
    def __init__(self, launcher, clock=time.monotonic, wait=wait_children,
                 entropy=lambda: secrets.token_hex(16), budget=15):
        if not 0 < budget <= 15:
            raise TransportFault('HOST_DEADLINE')
        self.launcher, self.clock, self.wait = launcher, clock, wait
        self.entropy, self.budget = entropy, budget
        self._used = False

    def run(self, text, cancelled=lambda: False):
        if self._used:
            return Outcome('REFUSED', 'REF_CONSUMED', 'UNVERIFIED', 'UNVERIFIED', True, False)
        self._used = True
        am = client = None
        am_eof = client_eof = False
        am_bad = client_bad = False
        committed = commit_flushed = False
        fault = None
        local_clean = True
        started = False
        bundle = wire = None
        children = []
        until = self.clock() + self.budget

        def fail(code):
            nonlocal fault
            if fault is None:
                fault = code
            if client is not None:
                client.close_input()  # Explicit EOF cancellation, never a successful half-close.

        def guarded():
            valid, failed = None, False
            try:
                valid = self.launcher.guard()
            except BaseException:
                failed = True
            if failed or valid is not None and valid is not True:
                raise TransportFault('GUARD_FAILED')
            if self.clock() >= until:
                raise TransportFault('HOST_DEADLINE')
            if cancelled():
                raise TransportFault('HOST_CANCELLED')

        try:
            append = PrivateAppend(text)  # Validate before any remote launch.
            instance = self.entropy()
            bundle, wire = Instrumentation(instance), Responses(instance)
            frames = InputFrames()
            guarded()
            am = self.launcher.instrumentation(instance)
            children.append(am)
            started = True
            while True:
                if self.clock() >= until:
                    fail('HOST_DEADLINE')
                    break
                if cancelled():
                    fail('HOST_CANCELLED')
                for child, is_am in ((am, True), (client, False)):
                    if child is None:
                        continue
                    error_output = child.read('stderr')
                    if error_output:
                        fail('STDERR_OUTPUT')
                    chunk = child.read('stdout')
                    if chunk is None:
                        continue
                    try:
                        if chunk:
                            if is_am and not am_bad:
                                bundle.feed(chunk)
                            elif not is_am and not client_bad:
                                wire.feed(chunk)
                        elif is_am:
                            am_eof = True
                            if not am_bad:
                                bundle.eof()
                        else:
                            client_eof = True
                            if not client_bad:
                                wire.eof()
                    except TransportFault as error:
                        if is_am:
                            am_bad = True
                        else:
                            client_bad = True
                        fail(error.args[0])
                if bundle.ready and client is None and fault is None:
                    guarded()
                    client = self.launcher.client(instance)
                    children.append(client)
                    client.queue(frames.observe())
                if client is not None:
                    if wire.reference is not None and not committed and not wire.channel_closed and fault is None:
                        guarded()
                        committed = True  # Never queue a second COMMIT or renew the device expiry.
                        client.queue(frames.commit(wire.reference, append))
                    if wire.channel_closed and committed and not commit_flushed:
                        fail('CHANNEL_LOST')
                    if wire.remote_exit is not None:
                        client.close_input()  # Only the remote-owner witness permits normal stdin closure.
                        if wire.remote_exit != 0:
                            fail('CLIENT_EXIT_NONZERO')
                    client.flush()
                    if committed and fault is None and not wire.channel_closed and client.pending_bytes() == 0:
                        commit_flushed = True
                if am_eof and client is None:
                    fail('CHANNEL_LOST')
                    break
                if am_eof and client_eof and all(child.exited() for child in children):
                    if not all(child.exit_ok() for child in children):
                        fail('CHANNEL_LOST')
                    break
                self.wait(children, min(.02, max(0, until - self.clock())))
            if fault is None:
                guarded()
        except TransportFault as error:
            fault = fault or error.args[0]
        except BaseException:
            fault = fault or 'HOST_FAILURE'
        finally:
            # The launcher's registry includes children whose setup raised before
            # return. Local kill/wait is containment only, never remote evidence.
            try:
                for child in self.launcher.owned():
                    if all(child is not known for known in children):
                        children.append(child)
            except BaseException:
                local_clean = False
            for child in reversed(children):
                try:
                    if not child.cleanup():
                        local_clean = False
                except BaseException:
                    local_clean = False
            if not local_clean:
                fault = fault or 'LOCAL_CLEANUP_FAILED'
        final = None if bundle is None or am_bad or not am_eof else bundle.final
        server_clean = final is not None and final[CLEANUP] == 'VERIFIED'
        client_clean = wire is not None and not client_bad and client_eof and wire.remote_exit is not None
        if final is not None and final[TEXT] == 'verified' and not commit_flushed:
            fault = fault or 'INVALID_BUNDLE'
        complete = started and server_clean and client_clean and local_clean and fault is None
        if complete:
            status = {'verified': 'VERIFIED', 'refused': 'REFUSED', 'uncertain': 'UNKNOWN'}[final[TEXT]]
            return Outcome(status, final[REASON], 'VERIFIED', 'REAPED', True, status != 'UNKNOWN')
        status = 'UNKNOWN' if started or children else 'REFUSED'
        return Outcome(status, fault or 'CLEANUP_UNVERIFIED',
                       'VERIFIED' if server_clean else 'UNVERIFIED',
                       'REAPED' if client_clean else 'UNVERIFIED', local_clean, False)
