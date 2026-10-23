//! Deterministic cancellation barriers around cooperative proof publication.
use super::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use tv_adapters::connection_proof::{Challenge, ProofStore};
use tv_domain::{Observed, Receipt};

struct Fixture {
    state: PathBuf,
    proof: BridgeProof,
    challenge: Challenge,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let state = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "tv-mcp-proof-barrier-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let snapshot = Arc::new(b"fixture configuration".to_vec());
        let challenge = ProofStore::open(&state.join("verification"))
            .unwrap()
            .issue(
                "fixture_installation",
                "fixture_revision",
                &snapshot,
                "screen",
                "generation_a",
                tv_adapters::runtime::SystemClock::default().unix_ms(),
            )
            .unwrap();
        Self {
            proof: BridgeProof {
                state: state.clone(),
                snapshot,
                generation: "generation_a".into(),
            },
            state,
            challenge,
        }
    }
    fn path(&self, name: &str) -> PathBuf {
        self.state.join("verification").join(name)
    }
    fn receipt(&self) -> Receipt {
        Receipt {
            request_id: self.challenge.nonce.clone(),
            status: Status::Observed,
            code: "observed",
            observed: Some(Observed {
                connected: true,
                ..Observed::default()
            }),
            local_elapsed_ms: 1,
            cached: false,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.state);
    }
}

struct CancelAtCheck {
    calls: AtomicUsize,
    cancel_at: usize,
    response: PathBuf,
}
impl Cancellation for CancelAtCheck {
    fn is_cancelled(&self) -> bool {
        let check = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        if check == self.cancel_at {
            if check == 2 {
                // This barrier proves that the second check happens after publication.
                assert!(self.response.is_file());
            }
            true
        } else {
            false
        }
    }
}

#[test]
fn cancelled_before_publication_does_not_create_a_response_or_return_a_code() {
    let fixture = Fixture::new();
    let cancellation = CancelAtCheck {
        calls: AtomicUsize::new(0),
        cancel_at: 1,
        response: fixture.path("response.json"),
    };
    let result = complete_verification(
        &fixture.proof,
        &fixture.challenge.nonce,
        &fixture.receipt(),
        &cancellation,
        fixture.challenge.expires_at_unix_ms,
    );
    assert!(matches!(result, Err("cancelled")));
    assert_eq!(cancellation.calls.load(Ordering::Relaxed), 1);
    assert!(!fixture.path("response.json").exists());
    assert!(fixture.path("request.json").is_file());
}

#[test]
fn cancellation_after_publication_removes_both_files_and_returns_no_code() {
    let fixture = Fixture::new();
    let cancellation = CancelAtCheck {
        calls: AtomicUsize::new(0),
        cancel_at: 2,
        response: fixture.path("response.json"),
    };
    let result = complete_verification(
        &fixture.proof,
        &fixture.challenge.nonce,
        &fixture.receipt(),
        &cancellation,
        fixture.challenge.expires_at_unix_ms,
    );
    assert!(matches!(result, Err("cancelled")));
    assert_eq!(cancellation.calls.load(Ordering::Relaxed), 2);
    assert!(!fixture.path("request.json").exists());
    assert!(!fixture.path("response.json").exists());
}

#[test]
fn expired_before_publication_does_not_create_a_response_or_return_a_code() {
    let fixture = Fixture::new();
    let cancellation = CancelAtCheck {
        calls: AtomicUsize::new(0),
        cancel_at: usize::MAX,
        response: fixture.path("response.json"),
    };
    let result = complete_verification(
        &fixture.proof,
        &fixture.challenge.nonce,
        &fixture.receipt(),
        &cancellation,
        0,
    );
    assert!(matches!(result, Err("expired")));
    assert_eq!(cancellation.calls.load(Ordering::Relaxed), 1);
    assert!(!fixture.path("response.json").exists());
    assert!(fixture.path("request.json").is_file());
}
