//! Cross-process leases and targeted cancellation using only a private local journal.
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use tv_adapters::operation_journal::OperationJournal;
use tv_application::generic::{Journal, Reservation};
use tv_domain::{
    generic::{Intent, Operation, Reply, Request},
    Status,
};

const WAIT: Duration = Duration::from_secs(5);
const PREFIX: &str = "JOURNAL_WORKER:";
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "tv-multi-process-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn journal(&self) -> OperationJournal {
        OperationJournal::open(&self.0.join("journal"), 16, 1000).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn request(id: &str, device: &str) -> Request {
    Request {
        id: id.into(),
        device: device.into(),
        intent: Intent::Pause,
        expires_at_unix_ms: 2000,
    }
}
fn reply(id: &str, status: Status) -> Reply {
    Reply {
        request_id: id.into(),
        status,
        code: "fixture_terminal".into(),
        observed: None,
        choices: vec![],
        cached: false,
        local_elapsed_ms: 0,
    }
}
fn event(value: &str) {
    println!("{PREFIX}{value}");
    std::io::stdout().flush().unwrap();
}

/// Invoked only as an explicitly owned child by this test binary.
#[test]
fn journal_worker() {
    let Some(path) = std::env::var_os("TV_MULTI_JOURNAL_DIRECTORY") else {
        return;
    };
    let id = std::env::var("TV_MULTI_JOURNAL_ID").unwrap();
    let resource = std::env::var("TV_MULTI_JOURNAL_RESOURCE").unwrap();
    let journal = OperationJournal::open(Path::new(&path), 16, 1000).unwrap();
    let req = request(&id, &id);
    assert!(matches!(
        journal.reserve(&req, &resource, 100).unwrap(),
        Reservation::Acquired
    ));
    event("READY");
    for line in std::io::stdin().lock().lines() {
        match line.unwrap().as_str() {
            "state" => event(if journal.is_cancelled(&id) {
                "CANCELLED"
            } else {
                "LIVE"
            }),
            "finish" => {
                let status = if journal.is_cancelled(&id) {
                    Status::Cancelled
                } else {
                    Status::Verified
                };
                journal.finish(&req, &reply(&id, status), 200).unwrap();
                event("DONE");
                return;
            }
            // Model interruption after possible dispatch without running destructors.
            "crash" => std::process::exit(0),
            _ => panic!("unexpected fixture command"),
        }
    }
    panic!("parent closed before terminal command");
}

struct Worker {
    child: Child,
    input: ChildStdin,
    events: mpsc::Receiver<String>,
    reader: Option<std::thread::JoinHandle<()>>,
}
impl Worker {
    fn start(temp: &Temp, id: &str, resource: &str) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "journal_worker", "--nocapture"])
            .env("TV_MULTI_JOURNAL_DIRECTORY", temp.0.join("journal"))
            .env("TV_MULTI_JOURNAL_ID", id)
            .env("TV_MULTI_JOURNAL_RESOURCE", resource)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, events) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().take(32) {
                let Ok(line) = line else {
                    break;
                };
                if line.len() > 512 {
                    break;
                }
                if let Some(value) = line.strip_prefix(PREFIX) {
                    if tx.send(value.to_owned()).is_err() {
                        break;
                    }
                }
            }
        });
        let worker = Self {
            child,
            input,
            events,
            reader: Some(reader),
        };
        worker.expect("READY");
        worker
    }
    fn send(&mut self, command: &str) {
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
    }
    fn expect(&self, value: &str) {
        assert_eq!(self.events.recv_timeout(WAIT).unwrap(), value);
    }
    fn exit(&mut self) {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            assert!(Instant::now() < deadline, "owned worker did not exit");
            // Only bounded reaping polling; test ordering is driven by pipe events.
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn finish(&mut self) {
        self.send("finish");
        self.expect("DONE");
        self.exit();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[test]
fn two_processes_keep_distinct_resources_and_cancellation_independent() {
    let temp = Temp::new();
    let mut a = Worker::start(&temp, "device-a", "resource-a");
    let mut b = Worker::start(&temp, "device-b", "resource-b");
    let journal = temp.journal();
    for id in ["device-a", "device-b"] {
        assert!(matches!(journal.get(id).unwrap(), Operation::Running));
    }
    assert!(matches!(
        journal
            .reserve(&request("alias", "alias-a"), "resource-a", 100)
            .unwrap(),
        Reservation::Busy
    ));
    assert!(matches!(
        journal.cancel("device-a").unwrap(),
        Operation::Running
    ));
    a.send("state");
    a.expect("CANCELLED");
    b.send("state");
    b.expect("LIVE");
    b.finish();
    assert!(
        matches!(journal.get("device-b").unwrap(), Operation::Complete(r) if r.status == Status::Verified)
    );
    assert!(matches!(
        journal.get("device-a").unwrap(),
        Operation::Running
    ));
    a.finish();
    drop(journal);
    let journal = temp.journal();
    for (id, resource, status) in [
        ("device-a", "resource-a", Status::Cancelled),
        ("device-b", "resource-b", Status::Verified),
    ] {
        let mut retry = request(id, id);
        retry.expires_at_unix_ms = 9000;
        assert!(
            matches!(journal.reserve(&retry, resource, 300).unwrap(), Reservation::Existing(Operation::Complete(r)) if r.status == status)
        );
        retry.device = "different-device".into();
        assert!(matches!(
            journal.reserve(&retry, resource, 300).unwrap(),
            Reservation::Conflict
        ));
    }
}

#[test]
fn crashed_owner_becomes_unknown_without_releasing_other_process_resource() {
    let temp = Temp::new();
    let mut a = Worker::start(&temp, "device-a", "resource-a");
    let mut b = Worker::start(&temp, "device-b", "resource-b");
    a.send("crash");
    a.exit();
    let journal = temp.journal();
    assert!(
        matches!(journal.get("device-a").unwrap(), Operation::Complete(r) if r.status == Status::OutcomeUnknown && r.code == "operation_interrupted")
    );
    assert!(matches!(
        journal.get("device-b").unwrap(),
        Operation::Running
    ));
    assert!(matches!(
        journal
            .reserve(&request("other-b", "alias-b"), "resource-b", 100)
            .unwrap(),
        Reservation::Busy
    ));
    b.send("state");
    b.expect("LIVE");
    // A new request may use A, but the interrupted request may never be replayed.
    let next = request("next-a", "device-a");
    assert!(matches!(
        journal.reserve(&next, "resource-a", 100).unwrap(),
        Reservation::Acquired
    ));
    journal
        .finish(&next, &reply("next-a", Status::Verified), 200)
        .unwrap();
    b.finish();
    drop(journal);
    let journal = temp.journal();
    assert!(
        matches!(journal.reserve(&request("device-a", "device-a"), "resource-a", 100_000).unwrap(), Reservation::Existing(Operation::Complete(r)) if r.status == Status::OutcomeUnknown)
    );
    assert!(
        matches!(journal.get("device-b").unwrap(), Operation::Complete(r) if r.status == Status::Verified)
    );
}
