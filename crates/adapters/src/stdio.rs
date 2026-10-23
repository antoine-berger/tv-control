//! Bounded nonblocking stdio mechanics for an incoming transport adapter.
use crate::process::nonblocking;
use serde_json::Value;
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct StdioFile {
    file: File,
    original_flags: i32,
}
impl Read for StdioFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}
impl Write for StdioFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
impl Drop for StdioFile {
    fn drop(&mut self) {
        // SAFETY: the descriptor is still owned here. Restore shared status flags
        // because dup/inherited stdio can share an open-file description with a caller.
        unsafe {
            libc::fcntl(self.file.as_raw_fd(), libc::F_SETFL, self.original_flags);
        }
    }
}
fn duplicate(fd: i32) -> io::Result<StdioFile> {
    // SAFETY: dup returns a new owned descriptor or -1; ownership is transferred once.
    let copied = unsafe { libc::dup(fd) };
    if copied < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(copied) };
    // SAFETY: the freshly duplicated descriptor is live and owned by file.
    let original_flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if original_flags < 0 {
        return Err(io::Error::last_os_error());
    }
    nonblocking(file.as_raw_fd())?;
    Ok(StdioFile {
        file,
        original_flags,
    })
}
pub enum Input {
    Frame(Vec<u8>),
    Idle,
    Closed,
    TooLarge,
}
pub struct FrameReader {
    file: StdioFile,
    pending: Vec<u8>,
    cursor: usize,
    frame: Vec<u8>,
}
impl FrameReader {
    pub fn stdin() -> io::Result<Self> {
        Ok(Self {
            file: duplicate(libc::STDIN_FILENO)?,
            pending: Vec::new(),
            cursor: 0,
            frame: Vec::new(),
        })
    }
    pub fn next(&mut self, max: usize) -> io::Result<Input> {
        loop {
            if self.cursor < self.pending.len() {
                let byte = self.pending[self.cursor];
                self.cursor += 1;
                self.frame.push(byte);
                if self.frame.len() > max {
                    return Ok(Input::TooLarge);
                }
                if byte == b'\n' {
                    return Ok(Input::Frame(std::mem::take(&mut self.frame)));
                }
                continue;
            }
            let mut block = [0u8; 4096];
            match self.file.read(&mut block) {
                Ok(0) => return Ok(Input::Closed),
                Ok(n) => {
                    self.pending.clear();
                    self.pending.extend_from_slice(&block[..n]);
                    self.cursor = 0;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(Input::Idle),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }
}
/// Separate allowance only for explicit native PNG image responses (8 MiB binary).
pub const MAX_IMAGE_FRAME_BYTES: usize = 12 * 1024 * 1024;

fn wait_writable(fd: libc::c_int, timeout: Duration) -> io::Result<()> {
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLOUT,
        revents: 0,
    };
    let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
    // SAFETY: descriptor is live for this call; poll only observes readiness.
    let result = unsafe { libc::poll(&mut descriptor, 1, millis) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    } else if descriptor.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "output pipe closed",
        ));
    }
    Ok(())
}

struct Frame {
    bytes: Vec<u8>,
    deadline: Instant,
}
pub struct Output {
    sender: SyncSender<Frame>,
    stop: Arc<AtomicBool>,
    max_frame: usize,
    timeout_ms: u64,
}
impl Output {
    pub fn start(
        capacity: usize,
        max_frame: usize,
        timeout_ms: u64,
        poll_ms: u64,
    ) -> io::Result<(Arc<Self>, thread::JoinHandle<()>)> {
        let mut file = duplicate(libc::STDOUT_FILENO)?;
        let (sender, receiver) = mpsc::sync_channel::<Frame>(capacity);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let handle = thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                let frame = match receiver.recv_timeout(Duration::from_millis(poll_ms.max(1))) {
                    Ok(frame) => frame,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let mut sent = 0;
                while sent < frame.bytes.len() {
                    if worker_stop.load(Ordering::Acquire) || Instant::now() >= frame.deadline {
                        worker_stop.store(true, Ordering::Release);
                        break;
                    }
                    match file.write(&frame.bytes[sent..]) {
                        Ok(0) => {
                            worker_stop.store(true, Ordering::Release);
                            break;
                        }
                        Ok(n) => sent += n,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            // Wait for actual pipe readiness rather than imposing a full poll
                            // interval per small pipe buffer (notably 16 KiB on macOS).
                            // The deadline and shutdown checks still bound every wait.
                            let timeout = Duration::from_millis(poll_ms.max(1))
                                .min(frame.deadline.saturating_duration_since(Instant::now()));
                            if wait_writable(file.file.as_raw_fd(), timeout).is_err() {
                                worker_stop.store(true, Ordering::Release);
                                break;
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => {
                            worker_stop.store(true, Ordering::Release);
                            break;
                        }
                    }
                }
            }
        });
        Ok((
            Arc::new(Self {
                sender,
                stop,
                max_frame,
                timeout_ms,
            }),
            handle,
        ))
    }
    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
    }
    pub fn send(&self, value: Value) {
        self.enqueue(value, self.timeout_ms, self.max_frame);
    }
    pub fn send_before(&self, value: Value, expires_at_unix_ms: u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        self.enqueue(
            value,
            self.timeout_ms.min(expires_at_unix_ms.saturating_sub(now)),
            self.max_frame,
        );
    }
    /// Only explicit, adapter-validated screenshots use this bounded image lane.
    /// The regular queue capacity, writer deadline and backpressure still apply.
    pub fn send_image_before(&self, value: Value, expires_at_unix_ms: u64) {
        let native_png = value.pointer("/result/content/0").is_some_and(|image| {
            image["type"] == "image"
                && image["mimeType"] == "image/png"
                && image["data"].is_string()
        });
        if !native_png {
            self.send_before(value, expires_at_unix_ms);
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        self.enqueue(
            value,
            self.timeout_ms.min(expires_at_unix_ms.saturating_sub(now)),
            MAX_IMAGE_FRAME_BYTES,
        );
    }
    fn enqueue(&self, value: Value, budget: u64, max_frame: usize) {
        if self.is_stopped() {
            return;
        }
        let Ok(mut bytes) = serde_json::to_vec(&value) else {
            self.shutdown();
            return;
        };
        bytes.push(b'\n');
        if bytes.len() > max_frame || budget == 0 {
            self.shutdown();
            return;
        }
        if self
            .sender
            .try_send(Frame {
                bytes,
                deadline: Instant::now() + Duration::from_millis(budget),
            })
            .is_err()
        {
            self.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn output() -> (Output, mpsc::Receiver<Frame>) {
        let (sender, receiver) = mpsc::sync_channel(1);
        (
            Output {
                sender,
                stop: Arc::new(AtomicBool::new(false)),
                max_frame: 4096,
                timeout_ms: 1000,
            },
            receiver,
        )
    }
    #[test]
    fn writable_wait_rejects_closed_descriptor() {
        assert!(wait_writable(i32::MAX, Duration::from_millis(10)).is_err());
    }
    #[test]
    fn writable_wait_wakes_when_reader_drains_and_bounds_stalls() {
        use std::os::unix::net::UnixStream;
        let (mut writer, mut reader) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        let bytes = [0u8; 8192];
        loop {
            match writer.write(&bytes) {
                Ok(_) => (),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("{e}"),
            }
        }
        let start = Instant::now();
        wait_writable(writer.as_raw_fd(), Duration::from_millis(20)).unwrap();
        assert!(start.elapsed() >= Duration::from_millis(10));
        let (tx, rx) = mpsc::channel();
        let draining = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            reader.set_nonblocking(true).unwrap();
            let mut bytes = [0u8; 8192];
            while reader.read(&mut bytes).is_ok_and(|n| n > 0) {}
            let _ = rx.recv_timeout(Duration::from_secs(2));
        });
        let start = Instant::now();
        wait_writable(writer.as_raw_fd(), Duration::from_millis(1000)).unwrap();
        assert!(start.elapsed() < Duration::from_millis(900));
        tx.send(()).unwrap();
        draining.join().unwrap();
    }
    fn image(size: usize) -> Value {
        json!({"result":{"content":[{"type":"image","mimeType":"image/png","data":"A".repeat(size.div_ceil(3)*4)}]}})
    }
    #[test]
    fn image_queue_preserves_explicit_limits_and_deadlines() {
        for size in [200 * 1024, 800 * 1024] {
            let (out, rx) = output();
            out.send_image_before(image(size), u64::MAX);
            let frame = rx.try_recv().unwrap();
            assert!(frame.bytes.len() > 4096 && frame.bytes.len() <= MAX_IMAGE_FRAME_BYTES);
            assert!(frame.deadline > Instant::now());
            assert!(!out.is_stopped());
        }
        let (out, rx) = output();
        out.send_image_before(image(200 * 1024), 0);
        assert!(out.is_stopped());
        assert!(rx.try_recv().is_err());
        let (out, rx) = output();
        out.send(image(200 * 1024));
        assert!(out.is_stopped());
        assert!(rx.try_recv().is_err());
        let (out, rx) = output();
        out.send_image_before(json!({"text":"x".repeat(5000)}), u64::MAX);
        assert!(out.is_stopped());
        assert!(rx.try_recv().is_err());
        let (out, rx) = output();
        out.send_image_before(image(MAX_IMAGE_FRAME_BYTES), u64::MAX);
        assert!(out.is_stopped());
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn image_queue_keeps_existing_backpressure() {
        let (out, _rx) = output();
        out.send_image_before(image(200 * 1024), u64::MAX);
        out.send_image_before(image(200 * 1024), u64::MAX);
        assert!(out.is_stopped());
    }
}
