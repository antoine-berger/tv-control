use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tv_adapters::connection_proof::{ProofStore, CHALLENGE_VALIDITY_MS};
use tv_domain::{Observed, Receipt, Status};

struct Fixture {
    root: PathBuf,
    store: ProofStore,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "tv-proof-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let store = ProofStore::open(&root.join("proof")).unwrap();
        Self { root, store }
    }
    fn request(&self) -> tv_adapters::connection_proof::Challenge {
        self.store
            .issue(
                "installation_a",
                "revision_a",
                b"fixture configuration",
                "screen",
                "generation_a",
                1000,
            )
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn receipt(nonce: &str) -> Receipt {
    Receipt {
        request_id: nonce.into(),
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

#[test]
fn requires_the_chat_return_code_and_consumes_a_success_once() {
    let fixture = Fixture::new();
    let request = fixture.request();
    assert_eq!(request.nonce.len(), 32);
    assert!(fixture
        .store
        .confirm(&request, "", b"fixture configuration", 1100)
        .is_err());
    let response = fixture
        .store
        .complete(
            &request.nonce,
            b"fixture configuration",
            "generation_a",
            &receipt(&request.nonce),
            1100,
        )
        .unwrap();
    assert_eq!(response.installation_id, "installation_a");
    assert_eq!(
        fixture
            .store
            .confirm(&request, "", b"fixture configuration", 1200)
            .err(),
        Some("proof_return_code_required")
    );
    assert_eq!(
        fixture
            .store
            .confirm(&request, &request.nonce, b"fixture configuration", 1200)
            .err(),
        Some("proof_return_code_required")
    );
    let confirmed = fixture
        .store
        .confirm(&request, &response.code, b"fixture configuration", 1200)
        .unwrap();
    assert_eq!(confirmed.observed_at_unix_ms, 1100);
    assert!(fixture
        .store
        .confirm(&request, &response.code, b"fixture configuration", 1200)
        .is_err());
    assert!(fixture
        .store
        .request(
            &request.nonce,
            b"fixture configuration",
            "generation_a",
            1200
        )
        .is_err());
    assert_eq!(fs::read_dir(fixture.root.join("proof")).unwrap().count(), 1);
}
#[test]
fn rejects_wrong_config_nonce_expiry_and_unsuitable_observations() {
    let fixture = Fixture::new();
    let request = fixture.request();
    assert_eq!(
        fixture
            .store
            .request(
                &request.nonce,
                b"changed configuration",
                "generation_a",
                1100
            )
            .err(),
        Some("proof_configuration_changed")
    );
    assert_eq!(
        fixture
            .store
            .request(
                &"0".repeat(32),
                b"fixture configuration",
                "generation_a",
                1100
            )
            .err(),
        Some("proof_challenge_mismatch")
    );
    for bad in 0..4 {
        let mut result = receipt(&request.nonce);
        match bad {
            0 => result.cached = true,
            1 => result.status = Status::Verified,
            2 => result.observed.as_mut().unwrap().connected = false,
            _ => result.request_id = "other".into(),
        }
        assert_eq!(
            fixture
                .store
                .complete(
                    &request.nonce,
                    b"fixture configuration",
                    "generation_a",
                    &result,
                    1100
                )
                .err(),
            Some("proof_observation_required")
        );
    }
    assert!(!fixture.root.join("proof/response.json").exists());
    assert_eq!(
        fixture
            .store
            .request(
                &request.nonce,
                b"fixture configuration",
                "generation_a",
                1000 + CHALLENGE_VALIDITY_MS
            )
            .err(),
        Some("proof_expired")
    );
    assert!(!fixture.root.join("proof/request.json").exists());
}
#[test]
fn a_new_journey_invalidates_old_proofs_and_cancel_cleans_only_its_own_request() {
    let fixture = Fixture::new();
    let first = fixture.request();
    let first_response = fixture
        .store
        .complete(
            &first.nonce,
            b"fixture configuration",
            "generation_a",
            &receipt(&first.nonce),
            1100,
        )
        .unwrap();
    let second = fixture
        .store
        .issue(
            "installation_a",
            "revision_b",
            b"new configuration",
            "screen",
            "generation_a",
            1200,
        )
        .unwrap();
    assert_ne!(first.nonce, second.nonce);
    assert!(fixture
        .store
        .confirm(&first, &first_response.code, b"fixture configuration", 1300)
        .is_err());
    assert_eq!(fixture.store.cancel(&first), Err("proof_binding_mismatch"));
    fixture.store.cancel(&second).unwrap();
    assert_eq!(fs::read_dir(fixture.root.join("proof")).unwrap().count(), 1);
}
#[test]
fn rejects_symlinks_hardlinks_unknown_entries_and_broad_permissions() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let path = fixture.root.join("proof/request.json");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let external = fixture.root.join("keep");
    fs::write(&external, b"preserve").unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&external, &path).unwrap();
    assert!(fixture
        .store
        .request(
            &request.nonce,
            b"fixture configuration",
            "generation_a",
            1100
        )
        .is_err());
    assert_eq!(fs::read(&external).unwrap(), b"preserve");
    fs::remove_file(&path).unwrap();
    fs::hard_link(&external, &path).unwrap();
    assert!(fixture
        .store
        .issue(
            "installation_a",
            "revision_a",
            b"config",
            "screen",
            "generation_a",
            1200
        )
        .is_err());
    fs::remove_file(&path).unwrap();
    fs::write(fixture.root.join("proof/unexpected"), b"preserve").unwrap();
    assert_eq!(
        fixture
            .store
            .issue(
                "installation_a",
                "revision_a",
                b"config",
                "screen",
                "generation_a",
                1200
            )
            .err(),
        Some("proof_unknown_entry")
    );
    fs::remove_file(fixture.root.join("proof/unexpected")).unwrap();
    fs::set_permissions(
        fixture.root.join("proof"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert_eq!(
        ProofStore::open(&fixture.root.join("proof"))
            .err()
            .map(|_| "not_private"),
        Some("not_private")
    );
}
#[test]
fn recovers_only_known_interrupted_atomic_publications() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let path = fixture.root.join("proof/request.json");
    fs::hard_link(
        &path,
        fixture
            .root
            .join(format!("proof/.proof-{}.tmp", "a".repeat(32))),
    )
    .unwrap();
    assert!(fixture
        .store
        .request(
            &request.nonce,
            b"fixture configuration",
            "generation_a",
            1100
        )
        .is_ok());
    assert_eq!(fs::read_dir(fixture.root.join("proof")).unwrap().count(), 2);
    let partial = fixture
        .root
        .join(format!("proof/.proof-{}.tmp", "b".repeat(32)));
    fs::write(&partial, b"incomplete").unwrap();
    fs::set_permissions(&partial, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(fixture
        .store
        .request(
            &request.nonce,
            b"fixture configuration",
            "generation_a",
            1100
        )
        .is_ok());
    assert!(!partial.exists());
}

#[test]
fn configuration_transition_invalidates_both_pending_challenge_and_response() {
    let fixture = Fixture::new();
    let challenge = fixture.request();
    let response = fixture
        .store
        .complete(
            &challenge.nonce,
            b"fixture configuration",
            "generation_a",
            &receipt(&challenge.nonce),
            1100,
        )
        .unwrap();
    fixture.store.invalidate().unwrap();
    assert!(fixture
        .store
        .confirm(&challenge, &response.code, b"fixture configuration", 1200,)
        .is_err());
    assert_eq!(fs::read_dir(fixture.root.join("proof")).unwrap().count(), 1);
    fixture.store.invalidate().unwrap();
}

#[test]
fn replacement_runtime_cannot_observe_or_complete_a_previous_generation_challenge() {
    let fixture = Fixture::new();
    let challenge = fixture.request();
    assert_eq!(
        fixture
            .store
            .request(
                &challenge.nonce,
                b"fixture configuration",
                "generation_b",
                1100,
            )
            .err(),
        Some("proof_runtime_changed")
    );
    assert_eq!(
        fixture
            .store
            .complete(
                &challenge.nonce,
                b"fixture configuration",
                "generation_b",
                &receipt(&challenge.nonce),
                1100,
            )
            .err(),
        Some("proof_runtime_changed")
    );
    assert!(!fixture.root.join("proof/response.json").exists());
}
