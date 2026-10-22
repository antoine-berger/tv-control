# Review corrections and remaining qualification

The first public commit was an early MVP. Independent source review identified
seven issues; follow-up regression coverage is part of this revision:

| Finding | Correction and evidence |
| --- | --- |
| Jellyfin missing/null position confused with native playback proof | Explicit PlaybackEvidence; adapter and use-case regressions, plus real stdio/HTTP missing/null/frozen/progress fixtures |
| Configured verification poll silently capped at 50 ms | Full wait implemented as cancellable slices; fake-clock 500 ms, cancellation and deadline tests |
| Blocked stdout prevented cancellation/teardown | Nonblocking bounded response writer and nonblocking reader; unread-output flood regression with bounded process exit |
| Aliases bypassed physical-device exclusion | Configured coordination groups and duplicate-target validation; concurrent two-alias reservation test |
| ADB client could kill a mismatched daemon | Read-only protocol preflight before connected invocation; fake incompatible server test. TOCTOU on concurrent server replacement remains a documented limitation, not a closed unconditional guarantee |
| Resume-media accepted any far-ahead progressing position | Configured seek tolerance plus elapsed-time bound; far-ahead and near-start/progress tests; fixture now actually applies the requested seek |
| Target package's tag could collide with another package | Global tag uniqueness required before monitor; duplicate-tag regression asserts zero mutation dispatch |

The macOS CI failure in the first commit came from a content-bound test sharing a
one-second Python startup budget. It now uses independent five-second contexts;
production deadlines and dedicated timeout tests were not weakened. The matrix
does not cancel its other jobs after one failure, preserving diagnostic evidence.

These changes require a new CI result and independent review. A crossbuild is not
a router execution test. The initial checked-in performance snapshot belongs to
the first commit and does not measure the new stdio/probe overhead. Hardware
qualification, main-chat integration and user-to-physical-effect latency remain
separate work. Do not infer completion from a successful fixture or dispatch.

The inherited-stdout regression checks both blocking and nonblocking input flags,
restoring the complete flags captured by the adapter. On macOS, a fresh pipe's
F_GETFL changes from 1 to 65537 after the first write, including a plain Python
os.write: XNU's kernel-only FWASWRITTEN (0x10000) records write history and is not
in the F_SETFL-settable FCNTLFLAGS. The test primes the pipe before taking its
baseline, then compares all bits without masking. This distinguishes kernel
history from a leaked O_NONBLOCK flag. Source:
https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/fcntl.h
