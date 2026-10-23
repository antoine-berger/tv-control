//! Controlling-terminal prompts independent of a piped installer; no secret echo.
use std::{
    cell::RefCell,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::fd::AsRawFd,
    sync::atomic::{AtomicBool, Ordering},
};
use tv_application::{Cancellation, Context};
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
extern "C" fn interrupt(_: i32) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}
pub struct Interrupts {
    old_int: libc::sigaction,
    old_term: libc::sigaction,
}
impl Interrupts {
    pub fn install() -> Result<Self, &'static str> {
        INTERRUPTED.store(false, Ordering::Relaxed);
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        let mut old_int = unsafe { std::mem::zeroed() };
        let mut old_term = unsafe { std::mem::zeroed() };
        action.sa_sigaction = interrupt as *const () as usize;
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(libc::SIGINT, &action, &mut old_int) != 0 {
                return Err("signal_setup_failed");
            }
            if libc::sigaction(libc::SIGTERM, &action, &mut old_term) != 0 {
                libc::sigaction(libc::SIGINT, &old_int, std::ptr::null_mut());
                return Err("signal_setup_failed");
            }
        }
        Ok(Self { old_int, old_term })
    }
}
impl Cancellation for Interrupts {
    fn is_cancelled(&self) -> bool {
        INTERRUPTED.load(Ordering::Relaxed)
    }
}
impl Drop for Interrupts {
    fn drop(&mut self) {
        unsafe {
            libc::sigaction(libc::SIGINT, &self.old_int, std::ptr::null_mut());
            libc::sigaction(libc::SIGTERM, &self.old_term, std::ptr::null_mut());
        }
    }
}
pub struct Terminal {
    file: RefCell<File>,
    flags: i32,
    original: libc::termios,
}
impl Terminal {
    pub fn open() -> Result<Self, &'static str> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(|_| "interactive_terminal_required")?;
        let mut original = unsafe { std::mem::zeroed() };
        let fd = file.as_raw_fd();
        if unsafe { libc::isatty(fd) } != 1 || unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            return Err("interactive_terminal_required");
        }
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err("terminal_setup_failed");
        }
        Ok(Self {
            file: RefCell::new(file),
            flags,
            original,
        })
    }
    pub fn line(
        &self,
        prompt: &str,
        secret: bool,
        ctx: &Context<'_>,
    ) -> Result<Option<String>, &'static str> {
        let mut file = self.file.borrow_mut();
        let fd = file.as_raw_fd();
        let mut settings = self.original;
        if secret {
            settings.c_lflag &= !libc::ECHO;
        }
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &settings) } != 0 {
            return Err("terminal_setup_failed");
        }
        struct Restore(i32, libc::termios, bool);
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe {
                    if self.2 {
                        // Canonical input can still contain a partial hidden key
                        // on cancellation. Discard it before restoring echo, but
                        // do not wait for output drain (as TCSAFLUSH would).
                        libc::tcflush(self.0, libc::TCIFLUSH);
                    }
                    libc::tcsetattr(self.0, libc::TCSANOW, &self.1);
                }
            }
        }
        let _restore = Restore(fd, self.original, secret);
        let mut left = prompt.as_bytes();
        while !left.is_empty() {
            ctx.check().map_err(|e| e.code)?;
            match file.write(left) {
                Ok(0) => return Err("terminal_closed"),
                Ok(n) => left = &left[n..],
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    ctx.wait(20).map_err(|e| e.code)?
                }
                Err(_) => return Err("terminal_write_failed"),
            }
        }
        let mut bytes = Vec::new();
        loop {
            ctx.check().map_err(|e| e.code)?;
            let mut byte = [0];
            match file.read(&mut byte) {
                Ok(0) => return Ok(None),
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) => {
                    bytes.push(byte[0]);
                    if bytes.len() > 4096 {
                        return Err("terminal_input_too_long");
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    ctx.wait(20).map_err(|e| e.code)?
                }
                Err(_) => return Err("terminal_read_failed"),
            }
        }
        if secret {
            let _ = file.write(b"\n");
        }
        let value = String::from_utf8(bytes).map_err(|_| "terminal_invalid_text")?;
        let value = value.trim().to_owned();
        Ok(if value == "q" { None } else { Some(value) })
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let fd = self.file.get_mut().as_raw_fd();
        unsafe {
            libc::tcsetattr(fd, libc::TCSANOW, &self.original);
            libc::fcntl(fd, libc::F_SETFL, self.flags);
        }
    }
}
