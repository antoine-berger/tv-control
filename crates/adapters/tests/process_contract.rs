use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tv_adapters::{
    process::{BoundedProcess, Invocation, Process},
    runtime::{CancelFlag, SystemClock},
};
use tv_application::{Clock, Context};
use tv_domain::Effect;
fn invocation(mutating: bool, script: &str) -> Invocation {
    Invocation {
        program: "/usr/bin/python3".into(),
        args: vec!["-c".into(), script.into()],
        env: BTreeMap::new(),
        stdin: Vec::new(),
        mutating,
    }
}
#[test]
fn deadline_kills_child_and_reports_unknown_after_mutation_spawn() {
    let p = BoundedProcess {
        max_output: 4096,
        poll_ms: 2,
    };
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    let ctx = Context {
        clock: &clock,
        cancellation: &flag,
        deadline_monotonic_ms: clock.monotonic_ms() + 40,
    };
    let start = Instant::now();
    let error = p
        .run(invocation(true, "import time; time.sleep(5)"), &ctx)
        .err()
        .unwrap();
    assert_eq!(error.code, "expired");
    assert_eq!(error.effect, Effect::MayHaveChanged);
    assert!(start.elapsed() < Duration::from_secs(1));
}
#[test]
fn cancellation_kills_running_read_without_claiming_mutation() {
    let p = BoundedProcess {
        max_output: 4096,
        poll_ms: 2,
    };
    let clock = SystemClock::default();
    let flag = Arc::new(CancelFlag::default());
    let other = flag.clone();
    let cancel = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        other.cancel();
    });
    let ctx = Context {
        clock: &clock,
        cancellation: &*flag,
        deadline_monotonic_ms: clock.monotonic_ms() + 5000,
    };
    let error = p
        .run(invocation(false, "import time; time.sleep(5)"), &ctx)
        .err()
        .unwrap();
    cancel.join().unwrap();
    assert_eq!(error.code, "cancelled");
    assert_eq!(error.effect, Effect::None);
}
#[test]
fn output_is_bounded_and_raw_stderr_is_not_returned() {
    let p = BoundedProcess {
        max_output: 1024,
        poll_ms: 2,
    };
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    let ctx = Context {
        clock: &clock,
        cancellation: &flag,
        deadline_monotonic_ms: clock.monotonic_ms() + 1000,
    };
    let e = p
        .run(invocation(false, "print('x'*100000)"), &ctx)
        .err()
        .unwrap();
    assert_eq!(e.code, "process_output_limit");
    let out = p
        .run(
            invocation(
                false,
                "import sys; print('safe'); print('private fixture', file=sys.stderr)",
            ),
            &ctx,
        )
        .unwrap();
    assert_eq!(out.stdout, b"safe\n");
}

#[test]
fn deadline_covers_blocked_stdin() {
    let p = BoundedProcess {
        max_output: 1_048_576,
        poll_ms: 2,
    };
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    let ctx = Context {
        clock: &clock,
        cancellation: &flag,
        deadline_monotonic_ms: clock.monotonic_ms() + 60,
    };
    let mut call = invocation(true, "import time; time.sleep(5)");
    call.stdin = vec![b'x'; 1_000_000];
    let start = Instant::now();
    let error = p.run(call, &ctx).err().unwrap();
    assert_eq!(error.code, "expired");
    assert_eq!(error.effect, Effect::MayHaveChanged);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn deadline_covers_descendant_holding_stdout_open() {
    let p = BoundedProcess {
        max_output: 4096,
        poll_ms: 2,
    };
    let clock = SystemClock::default();
    let flag = CancelFlag::default();
    let ctx = Context {
        clock: &clock,
        cancellation: &flag,
        deadline_monotonic_ms: clock.monotonic_ms() + 80,
    };
    let start = Instant::now();
    let error=p.run(invocation(false,"import subprocess; subprocess.Popen(['/usr/bin/python3','-c','import time; time.sleep(5)'])"),&ctx).err().unwrap();
    assert_eq!(error.code, "expired");
    assert!(start.elapsed() < Duration::from_secs(1));
}
