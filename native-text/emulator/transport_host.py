"""Lab-only two-channel supervision. Not a production capability or free-serial CLI."""
import secrets
import math
import time
from transport_bundle import CLEANUP, ENTERED, REASON, TEXT, Instrumentation
from transport_pipes import wait_children
from transport_diagnostics import ClientFailure
from transport_wire import InputFrames, PrivateAppend, Responses, TransportFault


class Outcome:
    def __init__(self, status, reason, server_cleanup, client_cleanup, local_cleanup, qualified, diagnostics=None):
        self.status, self.reason = status, reason
        self.server_cleanup, self.client_cleanup = server_cleanup, client_cleanup
        self.local_cleanup, self.qualified = local_cleanup, qualified
        self.diagnostics = diagnostics

    def public(self):
        return {'status': self.status, 'reason': self.reason,
                'server_cleanup': self.server_cleanup, 'client_cleanup': self.client_cleanup,
                'local_cleanup': self.local_cleanup, 'qualified': self.qualified,
                'cleanup': 'VERIFIED' if self.qualified else 'UNVERIFIED',
                'diagnostics': self.diagnostics}


class Host:
    """The injected launcher belongs to the existing, exclusively owned AVD runtime.

    guard(deadline) must revalidate the private ADB owner and pinned helper/user/UID/artifact
    identity. instrumentation(instance) and client(instance) launch only the reviewed
    fixed commands, registering each owned child before any operation can raise.
    owned() must retain every such channel, including a partially failed launch.
    It must never restart/connect another ADB server or accept an arbitrary serial.
    No real launcher or command-line adapter is selected by this module.
    """
    def __init__(self, launcher, clock=time.monotonic, wait=wait_children,
                 entropy=lambda: secrets.token_hex(16), budget=15, deadline=None):
        if not 0 < budget <= 15:
            raise TransportFault('HOST_DEADLINE')
        self.launcher, self.clock, self.wait = launcher, clock, wait
        self.entropy, self.budget = entropy, budget
        self.deadline = deadline
        self._used = False

    def run(self, text, cancelled=lambda: False):
        return self._run(text, cancelled, False)

    def probe_connectivity(self, cancelled=lambda: False):
        """Closed lab probe: CANCEL first, with no OBSERVE, editor capture or text."""
        return self._run(None, cancelled, True)

    def _run(self, text, cancelled, connectivity):
        if self._used:
            return Outcome('REFUSED', 'REF_CONSUMED', 'UNVERIFIED', 'UNVERIFIED', True, False)
        self._used = True
        am = client = None
        am_eof = client_eof = False
        am_err_eof = client_err_eof = False
        am_bad = client_bad = False
        committed = commit_flushed = cancel_flushed = False
        fault = None
        local_clean = True
        started = False
        bundle = wire = None
        children = []
        client_failure = None
        origin = self.clock()
        until = origin + self.budget
        timing = dict.fromkeys(('ready_received_ms', 'host_client_guard_start_ms',
                               'host_client_guard_end_ms', 'client_call_start_ms', 'client_call_end_ms'))
        timing_bad = False
        timing_last = origin
        ready_seen = False

        def stamp(key):
            nonlocal timing_bad, timing_last
            # Secondary observation only: never raise into operation control.
            try:
                now = self.clock()
                if (type(now) not in (int, float) or type(origin) not in (int, float)
                        or not math.isfinite(now) or not math.isfinite(origin) or now < timing_last):
                    timing_bad = True
                    return
                timing_last = now
                elapsed = (now - origin) * 1000
                if not math.isfinite(elapsed) or elapsed < 0:
                    timing_bad = True
                    return
                if elapsed > 15000:
                    timing_bad = True
                timing[key] = min(15000, round(elapsed))
            except BaseException:
                timing_bad = True
        if self.deadline is not None:
            until = min(until, self.deadline)

        def fail(code):
            nonlocal fault
            if fault is None:
                fault = code
            if client is not None:
                client.close_input()  # Explicit EOF cancellation, never a successful half-close.

        def guarded():
            valid, failed = None, False
            try:
                valid = self.launcher.guard(until)
            except BaseException:
                failed = True
            if failed or valid is not None and valid is not True:
                raise TransportFault('GUARD_FAILED')
            if self.clock() >= until:
                raise TransportFault('HOST_DEADLINE')
            if cancelled():
                raise TransportFault('HOST_CANCELLED')

        try:
            append = None if connectivity else PrivateAppend(text)  # Validate before any remote launch.
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
                        if not is_am:
                            client_failure.feed(error_output)
                        fail('STDERR_OUTPUT')
                    elif error_output == b'':
                        if is_am:
                            am_err_eof = True
                        else:
                            client_err_eof = True
                            client_failure.eof()
                    chunk = child.read('stdout')
                    if chunk is None:
                        continue
                    try:
                        if chunk:
                            if is_am and not am_bad:
                                bundle.feed(chunk)
                                if bundle.ready and not ready_seen:
                                    ready_seen = True
                                    stamp('ready_received_ms')
                            elif not is_am and not client_bad:
                                wire.feed(chunk)
                            elif not is_am:
                                wire.count_discarded(chunk)
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
                    stamp('host_client_guard_start_ms')
                    try:
                        guarded()
                    finally:
                        stamp('host_client_guard_end_ms')
                    client_failure = ClientFailure()
                    stamp('client_call_start_ms')
                    try:
                        client = self.launcher.client(instance)
                    finally:
                        stamp('client_call_end_ms')
                    children.append(client)
                    client.queue(frames.cancel() if connectivity else frames.observe())
                if client is not None:
                    if connectivity and wire.reference is not None:
                        fail('INVALID_FRAME')
                    if not connectivity and wire.reference is not None and not committed and not wire.channel_closed and fault is None:
                        guarded()
                        committed = True  # Never queue a second COMMIT or renew the device expiry.
                        client.queue(frames.commit(wire.reference, append))
                    if wire.channel_closed and (committed and not commit_flushed
                                                or connectivity and not cancel_flushed):
                        fail('CHANNEL_LOST')
                    if wire.remote_exit is not None:
                        client.close_input()  # Only the remote-owner witness permits normal stdin closure.
                        if wire.remote_exit != 0:
                            fail('CLIENT_EXIT_NONZERO')
                    client.flush()
                    if connectivity and fault is None and not wire.channel_closed and client.pending_bytes() == 0:
                        cancel_flushed = True
                    if committed and fault is None and not wire.channel_closed and client.pending_bytes() == 0:
                        commit_flushed = True
                if am_eof and client is None:
                    fail('CHANNEL_LOST')
                    break
                if am_eof and client_eof and am_err_eof and client_err_eof and all(child.exited() for child in children):
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
        # Cleanup can consume the remaining budget or race caller cancellation.
        # Recheck without touching pipes that have already been closed/reaped.
        try:
            if self.clock() >= until:
                fault = fault or 'HOST_DEADLINE'
            elif cancelled():
                fault = fault or 'HOST_CANCELLED'
        except BaseException:
            fault = fault or 'HOST_FAILURE'
        final = None if bundle is None or am_bad or not am_eof else bundle.final
        server_clean = final is not None and final[CLEANUP] == 'VERIFIED'
        client_clean = wire is not None and not client_bad and client_eof and wire.remote_exit is not None
        if connectivity and final is not None and (not cancel_flushed or wire.reference is not None
                or final[TEXT] != 'refused' or final[REASON] != 'CANCELLED' or final[ENTERED] != 'NO'):
            fault = fault or 'INVALID_BUNDLE'
        if final is not None and final[TEXT] == 'verified' and not commit_flushed:
            fault = fault or 'INVALID_BUNDLE'
        launcher_timing = dict(guard_ms=None, popen_ms=None, incomplete=True)
        try:
            observed_timing = self.launcher.client_timing
            if (type(observed_timing) is dict and observed_timing.keys() == launcher_timing.keys()
                    and type(observed_timing['incomplete']) is bool
                    and all(value is None or type(value) is int and 0 <= value <= 15000
                            for key, value in observed_timing.items() if key != 'incomplete')):
                launcher_timing = dict(observed_timing)
        except BaseException:
            pass
        timing_incomplete = (timing_bad or any(value is None for value in timing.values())
            or launcher_timing['incomplete'] or any(launcher_timing[key] is None for key in ('guard_ms', 'popen_ms')))
        diagnostics = dict(server_ready=bundle is not None and bundle.ready,
                           server_final=None if final is None else dict(text=final[TEXT], cleanup=final[CLEANUP],
                               entered=final[ENTERED], reason=final[REASON]),
                           response=None if wire is None else wire.diagnostics(),
                           client_stderr=None if client_failure is None else client_failure.public(),
                           timing=dict(origin='HOST_RUN_MONOTONIC', host=dict(timing),
                                       launcher=launcher_timing, incomplete=timing_incomplete))
        complete = started and server_clean and client_clean and local_clean and fault is None
        if complete:
            status = 'CONNECTED' if connectivity else {
                'verified': 'VERIFIED', 'refused': 'REFUSED', 'uncertain': 'UNKNOWN'}[final[TEXT]]
            return Outcome(status, final[REASON], 'VERIFIED', 'REAPED', True, status != 'UNKNOWN', diagnostics)
        status = 'UNKNOWN' if started or children else 'REFUSED'
        return Outcome(status, fault or 'CLEANUP_UNVERIFIED',
                       'VERIFIED' if server_clean else 'UNVERIFIED',
                       'REAPED' if client_clean else 'UNVERIFIED', local_clean, False, diagnostics)
