use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tv_adapters::operation_journal::OperationJournal;
use tv_application::generic::{Journal, Reservation};
use tv_domain::{
    generic::{Intent, Operation, Reply, Request},
    Status,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "tv-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        Self(p)
    }
    fn journal(&self, capacity: usize) -> OperationJournal {
        OperationJournal::open(&self.0.join("journal"), capacity, 1000).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn request(id: &str) -> Request {
    Request {
        id: id.into(),
        device: "living".into(),
        intent: Intent::OpenApp {
            app: "YouTube".into(),
        },
        expires_at_unix_ms: 2000,
    }
}
fn reply(id: &str, status: Status) -> Reply {
    Reply {
        request_id: id.into(),
        status,
        code: "test".into(),
        observed: None,
        choices: vec![],
        cached: false,
        local_elapsed_ms: 4,
    }
}

#[test]
fn completed_outcome_survives_restart_and_renewed_expiry() {
    let t = Temp::new();
    let j = t.journal(4);
    let r = request("a");
    assert!(matches!(
        j.reserve(&r, "tv", 100).unwrap(),
        Reservation::Acquired
    ));
    j.finish(&r, &reply("a", Status::Verified), 200).unwrap();
    drop(j);
    let j = t.journal(4);
    let mut retry = r;
    retry.expires_at_unix_ms = 9000;
    assert!(
        matches!(j.reserve(&retry, "tv", 300).unwrap(), Reservation::Existing(Operation::Complete(v)) if v.status == Status::Verified)
    );
}
#[test]
fn abandoned_operation_is_durable_unknown_not_replayed_or_evicted() {
    let t = Temp::new();
    let j = t.journal(1);
    let r = request("a");
    assert!(matches!(
        j.reserve(&r, "tv", 100).unwrap(),
        Reservation::Acquired
    ));
    drop(j);
    let j = t.journal(1);
    assert!(
        matches!(j.get("a").unwrap(), Operation::Complete(v) if v.status == Status::OutcomeUnknown && v.code == "operation_interrupted")
    );
    assert!(
        matches!(j.reserve(&r, "tv", 100000).unwrap(), Reservation::Existing(Operation::Complete(v)) if v.status == Status::OutcomeUnknown)
    );
    assert_eq!(
        j.reserve(&request("b"), "tv", 100000).err().unwrap().code,
        "operation_journal_capacity"
    );
}
#[test]
fn multiple_instances_share_liveness_exclusion_cancellation_and_result() {
    let t = Temp::new();
    let a = t.journal(4);
    let b = t.journal(4);
    let r = request("a");
    assert!(matches!(
        a.reserve(&r, "physical-tv", 100).unwrap(),
        Reservation::Acquired
    ));
    assert!(matches!(b.get("a").unwrap(), Operation::Running));
    let mut other = request("b");
    other.device = "other_alias".into();
    assert!(matches!(
        b.reserve(&other, "physical-tv", 100).unwrap(),
        Reservation::Busy
    ));
    assert!(matches!(b.cancel("a").unwrap(), Operation::Running));
    assert!(a.is_cancelled("a"));
    a.finish(&r, &reply("a", Status::Cancelled), 200).unwrap();
    assert!(matches!(b.get("a").unwrap(), Operation::Complete(v) if v.status == Status::Cancelled));
    assert!(matches!(
        b.reserve(&other, "physical-tv", 300).unwrap(),
        Reservation::Acquired
    ));
}
#[test]
fn conflicts_bind_device_intent_and_resource() {
    let t = Temp::new();
    let j = t.journal(4);
    let r = request("a");
    j.reserve(&r, "tv", 0).unwrap();
    let mut changed = r.clone();
    changed.device = "bedroom".into();
    assert!(matches!(
        j.reserve(&changed, "tv", 0).unwrap(),
        Reservation::Conflict
    ));
    changed = r.clone();
    changed.intent = Intent::Pause;
    assert!(matches!(
        j.reserve(&changed, "tv", 0).unwrap(),
        Reservation::Conflict
    ));
    assert!(matches!(
        j.reserve(&r, "other", 0).unwrap(),
        Reservation::Conflict
    ));
    assert_eq!(
        j.finish(&changed, &reply("a", Status::Verified), 1)
            .unwrap_err()
            .code,
        "operation_conflict"
    );
}
#[test]
fn known_outcomes_expire_and_storage_is_bounded() {
    let t = Temp::new();
    let j = t.journal(1);
    let a = request("a");
    j.reserve(&a, "tv", 0).unwrap();
    j.finish(&a, &reply("a", Status::Verified), 1).unwrap();
    assert_eq!(
        j.reserve(&request("b"), "tv", 500).err().unwrap().code,
        "operation_journal_capacity"
    );
    assert!(matches!(
        j.reserve(&request("b"), "tv", 1002).unwrap(),
        Reservation::Acquired
    ));
    assert!(matches!(j.get("a").unwrap(), Operation::Missing));
    assert!(!t.0.join("journal/lease-a").exists());
}
#[test]
fn unsafe_directory_and_symlink_state_are_refused() {
    let t = Temp::new();
    let bad = t.0.join("public");
    fs::create_dir(&bad).unwrap();
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(OperationJournal::open(&bad, 4, 1000).is_err());
    let j = t.journal(4);
    drop(j);
    let outside = t.0.join("outside");
    fs::write(&outside, b"untouched").unwrap();
    symlink(&outside, t.0.join("journal/operations.json")).unwrap();
    assert!(OperationJournal::open(&t.0.join("journal"), 4, 1000).is_err());
    assert_eq!(fs::read(outside).unwrap(), b"untouched");
}
#[test]
fn corrupt_state_is_not_repaired_or_overwritten() {
    let t = Temp::new();
    let j = t.journal(4);
    let path = t.0.join("journal/operations.json");
    fs::write(&path, b"broken").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        j.reserve(&request("a"), "tv", 0).err().unwrap().code,
        "operation_journal_corrupt"
    );
    assert_eq!(fs::read(path).unwrap(), b"broken");
    assert!(j.is_cancelled("a"));
}
#[test]
fn unowned_finish_cannot_forge_a_success() {
    let t = Temp::new();
    let a = t.journal(4);
    let b = t.journal(4);
    let r = request("a");
    a.reserve(&r, "tv", 0).unwrap();
    assert_eq!(
        b.finish(&r, &reply("a", Status::Verified), 1)
            .unwrap_err()
            .code,
        "operation_not_owned"
    );
    assert!(matches!(b.get("a").unwrap(), Operation::Running));
}
#[test]
fn unknown_ids_and_invalid_ids_do_not_create_reservations() {
    let t = Temp::new();
    let j = t.journal(4);
    assert!(matches!(j.get("missing").unwrap(), Operation::Missing));
    assert!(matches!(j.cancel("missing").unwrap(), Operation::Missing));
    assert!(j.get("../escape").is_err());
    assert!(j.reserve(&request("../escape"), "tv", 0).is_err());
}

#[test]
fn journal_child_process() {
    let Some(path) = std::env::var_os("TV_JOURNAL_CRASH_TEST_DIRECTORY") else {
        return;
    };
    let j = OperationJournal::open(std::path::Path::new(&path), 4, 1000).unwrap();
    assert!(matches!(
        j.reserve(&request("crashed"), "tv", 100).unwrap(),
        Reservation::Acquired
    ));
    // Exit without Rust destructors, as with process termination after dispatch.
    std::process::exit(0);
}
#[test]
fn subprocess_exit_releases_lease_but_preserves_unknown_tombstone() {
    let t = Temp::new();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "journal_child_process", "--nocapture"])
        .env("TV_JOURNAL_CRASH_TEST_DIRECTORY", t.0.join("journal"))
        .status()
        .unwrap();
    assert!(status.success());
    let j = t.journal(4);
    assert!(
        matches!(j.get("crashed").unwrap(), Operation::Complete(v) if v.status == Status::OutcomeUnknown)
    );
    assert!(
        matches!(j.reserve(&request("crashed"), "tv", 100).unwrap(), Reservation::Existing(Operation::Complete(v)) if v.status == Status::OutcomeUnknown)
    );
}
#[test]
fn hardlinked_state_is_refused_without_touching_other_link() {
    let t = Temp::new();
    let j = t.journal(4);
    let r = request("a");
    j.reserve(&r, "tv", 0).unwrap();
    j.finish(&r, &reply("a", Status::Verified), 1).unwrap();
    let state = t.0.join("journal/operations.json");
    let link = t.0.join("extra");
    fs::hard_link(&state, &link).unwrap();
    let before = fs::read(&state).unwrap();
    assert_eq!(
        j.get("a").unwrap_err().code,
        "operation_journal_not_private"
    );
    assert_eq!(fs::read(link).unwrap(), before);
}
#[test]
fn journal_files_have_private_modes() {
    let t = Temp::new();
    let j = t.journal(4);
    j.reserve(&request("a"), "tv", 0).unwrap();
    for file in ["transaction.lock", "operations.json", "lease-a"] {
        assert_eq!(
            fs::metadata(t.0.join("journal").join(file))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn failed_finish_does_not_leave_immortal_running_owner() {
    use std::os::fd::AsRawFd;
    let t = Temp::new();
    let j = t.journal(4);
    let r = request("a");
    j.reserve(&r, "tv", 0).unwrap();
    let lock = fs::File::open(t.0.join("journal/transaction.lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert_eq!(
        j.finish(&r, &reply("a", Status::Verified), 1)
            .unwrap_err()
            .code,
        "operation_journal_busy"
    );
    drop(lock);
    assert!(
        matches!(j.get("a").unwrap(), Operation::Complete(v) if v.status == Status::OutcomeUnknown)
    );
}

#[test]
fn media_command_and_target_are_durable_request_identity() {
    use tv_domain::generic::MediaCommand;
    let temp = Temp::new();
    let journal = temp.journal(4);
    let mut r = request("media");
    r.intent = Intent::Media {
        session: "org.example.player/player".into(),
        command: MediaCommand::Next,
    };
    assert!(matches!(
        journal.reserve(&r, "tv", 100).unwrap(),
        Reservation::Acquired
    ));
    journal
        .finish(&r, &reply("media", Status::Observed), 200)
        .unwrap();
    drop(journal);
    let journal = temp.journal(4);
    assert!(matches!(
        journal.reserve(&r, "tv", 300).unwrap(),
        Reservation::Existing(Operation::Complete(_))
    ));
    for intent in [
        Intent::Media {
            session: "org.example.player/player".into(),
            command: MediaCommand::Previous,
        },
        Intent::Media {
            session: "org.example.other/player".into(),
            command: MediaCommand::Next,
        },
    ] {
        r.intent = intent;
        assert!(matches!(
            journal.reserve(&r, "tv", 300).unwrap(),
            Reservation::Conflict
        ));
    }
}

#[test]
fn text_intent_survives_restart_and_conflicts_on_changed_target_or_text() {
    let t = Temp::new();
    let j = t.journal(4);
    let mut r = request("text");
    r.intent = Intent::TypeText {
        reference: "snapshot:1:0".into(),
        text: "title".into(),
    };
    assert!(matches!(
        j.reserve(&r, "tv", 100).unwrap(),
        Reservation::Acquired
    ));
    j.finish(&r, &reply("text", Status::Observed), 200).unwrap();
    drop(j);
    let j = t.journal(4);
    assert!(
        matches!(j.reserve(&r, "tv", 300).unwrap(), Reservation::Existing(Operation::Complete(v)) if v.status == Status::Observed)
    );
    for (reference, text) in [("snapshot:1:0", "different"), ("snapshot:2:0", "title")] {
        let mut changed = r.clone();
        changed.intent = Intent::TypeText {
            reference: reference.into(),
            text: text.into(),
        };
        assert!(matches!(
            j.reserve(&changed, "tv", 300).unwrap(),
            Reservation::Conflict
        ));
    }
}

#[test]
fn remote_navigation_serializes_across_instances_and_survives_restart() {
    use tv_domain::generic::Key;
    let t = Temp::new();
    let a = t.journal(4);
    let b = t.journal(4);
    let mut long = request("long");
    long.intent = Intent::LongSelect;
    let mut menu = request("menu");
    menu.intent = Intent::Navigate {
        keys: vec![Key::Menu, Key::Settings],
    };
    assert!(matches!(
        a.reserve(&long, "tv", 100).unwrap(),
        Reservation::Acquired
    ));
    assert!(matches!(
        b.reserve(&menu, "tv", 100).unwrap(),
        Reservation::Busy
    ));
    a.finish(&long, &reply("long", Status::OutcomeUnknown), 200)
        .unwrap();
    assert!(matches!(
        b.reserve(&menu, "tv", 200).unwrap(),
        Reservation::Acquired
    ));
    b.finish(&menu, &reply("menu", Status::Observed), 201)
        .unwrap();
    drop(a);
    drop(b);
    let j = t.journal(4);
    assert!(
        matches!(j.reserve(&long, "tv", 300).unwrap(), Reservation::Existing(Operation::Complete(r)) if r.status == Status::OutcomeUnknown)
    );
    assert!(
        matches!(j.reserve(&menu, "tv", 300).unwrap(), Reservation::Existing(Operation::Complete(r)) if r.status == Status::Observed)
    );
    menu.intent = Intent::Navigate {
        keys: vec![Key::Menu],
    };
    assert!(matches!(
        j.reserve(&menu, "tv", 300).unwrap(),
        Reservation::Conflict
    ));
}

#[test]
fn old_shape_journal_state_upgrades_without_a_source_and_none_is_not_written() {
    let t = Temp::new();
    let j = t.journal(4);
    let r = request("old-state");
    j.reserve(&r, "tv", 100).unwrap();
    let mut result = reply("old-state", Status::Verified);
    result.observed = Some(tv_domain::Observed {
        connected: true,
        ..Default::default()
    });
    j.finish(&r, &result, 200).unwrap();
    let raw = fs::read_to_string(t.0.join("journal/operations.json")).unwrap();
    assert!(
        !raw.contains("media_source_id"),
        "None must preserve the old serialized state shape"
    );
    drop(j);
    let j = t.journal(4);
    let Operation::Complete(result) = j.get("old-state").unwrap() else {
        panic!()
    };
    assert_eq!(result.observed.unwrap().media_source_id, None);
}

#[test]
fn abandoned_standby_is_never_replayed_and_conflicts_with_wake() {
    let temp = Temp::new();
    let mut request = request("standby");
    request.intent = Intent::Standby;
    {
        let journal = temp.journal(4);
        assert!(matches!(
            journal.reserve(&request, "tv", 100).unwrap(),
            Reservation::Acquired
        ));
    }
    let journal = temp.journal(4);
    assert!(matches!(
        journal.reserve(&request, "tv", 200).unwrap(),
        Reservation::Existing(Operation::Complete(r)) if r.status == Status::OutcomeUnknown
    ));
    request.intent = Intent::Wake;
    assert!(matches!(
        journal.reserve(&request, "tv", 200).unwrap(),
        Reservation::Conflict
    ));
}

#[test]
fn relative_volume_identity_and_unknown_receipt_survive_restart() {
    for (intent, changed) in [
        (
            Intent::AdjustVolume { delta: -3 },
            Intent::AdjustVolume { delta: -2 },
        ),
        (
            Intent::SetVolumePercent { percent: 33 },
            Intent::SetVolumePercent { percent: 34 },
        ),
    ] {
        let t = Temp::new();
        let j = t.journal(4);
        let mut r = request("relative");
        r.intent = intent;
        assert!(matches!(
            j.reserve(&r, "tv", 100).unwrap(),
            Reservation::Acquired
        ));
        j.finish(&r, &reply("relative", Status::OutcomeUnknown), 200)
            .unwrap();
        drop(j);
        let j = t.journal(4);
        r.expires_at_unix_ms = 9000;
        assert!(
            matches!(j.reserve(&r, "tv", 300).unwrap(), Reservation::Existing(Operation::Complete(v)) if v.status == Status::OutcomeUnknown)
        );
        r.intent = changed;
        assert!(matches!(
            j.reserve(&r, "tv", 300).unwrap(),
            Reservation::Conflict
        ));
    }
}
