//! Unix bounded subprocess adapter. Nonblocking pipes keep stdin, stdout, stderr,
//! exit and inherited pipes inside the same deadline, without per-pipe threads.
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::process::CommandExt,
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
};
use tv_application::Context;
use tv_domain::*;

pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: BTreeMap<OsString, OsString>,
    pub stdin: Vec<u8>,
    pub mutating: bool,
}
pub struct Output {
    pub stdout: Vec<u8>,
    pub success: bool,
}
pub trait Process: Send + Sync {
    fn run(&self, invocation: Invocation, ctx: &Context<'_>) -> PortResult<Output>;
}
pub struct BoundedProcess {
    pub max_output: usize,
    pub poll_ms: u64,
}

struct OwnedGroup(Child);
impl Drop for OwnedGroup {
    fn drop(&mut self) {
        // SAFETY: process_group(0) created an invocation-owned group; no caller-supplied PID.
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: fd is borrowed from a live owned Child pipe; fcntl does not retain it.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn read_available(
    reader: &mut impl Read,
    mut capture: Option<&mut Vec<u8>>,
    count: &mut usize,
    limit: usize,
    ctx: &Context<'_>,
) -> Result<bool, &'static str> {
    let mut block = [0u8; 4096];
    loop {
        ctx.check().map_err(|e| e.code)?;
        match reader.read(&mut block) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                *count = count.saturating_add(n);
                if *count > limit {
                    return Err("process_output_limit");
                }
                if let Some(bytes) = capture.as_deref_mut() {
                    bytes.extend_from_slice(&block[..n]);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("process_pipe_failed"),
        }
    }
}
impl Process for BoundedProcess {
    fn run(&self, invocation: Invocation, ctx: &Context<'_>) -> PortResult<Output> {
        ctx.check()?;
        if invocation.stdin.len() > self.max_output {
            return Err(Failure::before("process_input_limit"));
        }
        let mut cmd = Command::new(invocation.program);
        cmd.args(invocation.args)
            .env_clear()
            .envs(invocation.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut owned = OwnedGroup(
            cmd.spawn()
                .map_err(|_| Failure::before("process_spawn_failed"))?,
        );
        let failure = |code| {
            if invocation.mutating {
                Failure::uncertain(code)
            } else {
                Failure::before(code)
            }
        };
        let mut input = Some(owned.0.stdin.take().ok_or(failure("process_pipe_failed"))?);
        let mut stdout = owned
            .0
            .stdout
            .take()
            .ok_or(failure("process_pipe_failed"))?;
        let mut stderr = owned
            .0
            .stderr
            .take()
            .ok_or(failure("process_pipe_failed"))?;
        for fd in [
            input
                .as_ref()
                .ok_or(failure("process_pipe_failed"))?
                .as_raw_fd(),
            stdout.as_raw_fd(),
            stderr.as_raw_fd(),
        ] {
            nonblocking(fd).map_err(|_| failure("process_pipe_failed"))?;
        }
        let mut sent = 0;
        let mut bytes = Vec::new();
        let mut count = 0;
        let mut status = None;
        loop {
            ctx.check().map_err(|e| failure(e.code))?;
            if let Some(pipe) = input.as_mut() {
                if sent == invocation.stdin.len() {
                    input = None;
                } else {
                    match pipe.write(&invocation.stdin[sent..]) {
                        Ok(0) => return Err(failure("process_input_failed")),
                        Ok(n) => sent += n,
                        Err(e)
                            if e.kind() == io::ErrorKind::WouldBlock
                                || e.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => return Err(failure("process_input_failed")),
                    }
                }
            }
            let out_closed = read_available(
                &mut stdout,
                Some(&mut bytes),
                &mut count,
                self.max_output,
                ctx,
            )
            .map_err(failure)?;
            let err_closed = read_available(&mut stderr, None, &mut count, self.max_output, ctx)
                .map_err(failure)?;
            if status.is_none() {
                status = owned
                    .0
                    .try_wait()
                    .map_err(|_| failure("process_wait_failed"))?;
            }
            if let Some(exit) = status {
                if out_closed && err_closed {
                    ctx.check().map_err(|e| failure(e.code))?;
                    return Ok(Output {
                        stdout: bytes,
                        success: exit.success(),
                    });
                }
            }
            ctx.clock
                .sleep_ms(self.poll_ms.max(1).min(ctx.remaining_ms()));
        }
    }
}
