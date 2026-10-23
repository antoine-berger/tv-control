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

#[test]
fn generic_tools_paginate_without_exceeding_output_budget() {
    let mut list = tools();
    crate::generic_mcp::extend_tools(&mut list);
    list["tools"].as_array_mut().unwrap().push(
        json!({"name":"extra","description":"x".repeat(1000),"inputSchema":{"type":"object"}}),
    );
    let first = tools_page(json!(1), list.clone(), None, 4096).unwrap();
    assert!(first.to_string().len() < 4096);
    let cursor = first.pointer("/result/nextCursor").unwrap();
    let second = tools_page(json!(2), list, Some(cursor), 4096).unwrap();
    assert!(second.to_string().len() < 4096);
    assert!(second.pointer("/result/nextCursor").is_none());
}
#[test]
fn large_generic_lists_are_explicitly_truncated_not_silently_dropped() {
    let items = (0..32)
        .map(|n| json!({"id":n,"label":"x".repeat(200)}))
        .collect::<Vec<_>>();
    let frame = bounded_tool_result(json!(1), json!({"apps":items}), false, 4096);
    assert!(frame.to_string().len() < 4096);
    assert_eq!(
        frame.pointer("/result/structuredContent/truncated"),
        Some(&json!(true))
    );
    assert!(!frame["result"]["structuredContent"]["apps"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn screenshot_is_native_image_only_and_respects_output_budget() {
    let image = crate::generic_mcp::Response {
        value: json!({"width":1,"height":1,"format":"png"}),
        failed: false,
        image: Some(vec![1, 2, 3]),
    };
    let frame = generic_tool_result(json!(1), image, 4096);
    assert_eq!(frame["result"]["content"][0]["type"], "image");
    assert_eq!(frame["result"]["content"][0]["data"], "AQID");
    assert!(frame["result"]["structuredContent"].get("data").is_none());
    let huge = crate::generic_mcp::Response {
        value: json!({"width":100,"height":100}),
        failed: false,
        image: Some(vec![0; 8 * 1024 * 1024 + 1]),
    };
    let limited = generic_tool_result(json!(2), huge, 4096);
    assert_eq!(
        limited["result"]["structuredContent"]["code"],
        "screenshot_too_large"
    );
    assert_eq!(limited["result"]["isError"], true);
    for (bytes, expected) in [
        (b"".as_slice(), ""),
        (b"f".as_slice(), "Zg=="),
        (b"fo".as_slice(), "Zm8="),
        (b"foo".as_slice(), "Zm9v"),
    ] {
        assert_eq!(base64(bytes), expected);
    }
}

#[test]
fn realistic_screenshot_sizes_use_only_the_bounded_image_allowance() {
    for (size, width, height) in [
        (200 * 1024, 1920, 1080),
        (2 * 1024 * 1024, 3840, 2160),
        (8 * 1024 * 1024, 3840, 2160),
    ] {
        let frame = generic_tool_result(
            json!(1),
            crate::generic_mcp::Response {
                value: json!({"width":width,"height":height,"format":"png"}),
                failed: false,
                image: Some(vec![0; size]),
            },
            4096,
        );
        assert_eq!(frame["result"]["isError"], false);
        assert_eq!(
            frame["result"]["content"][0]["data"]
                .as_str()
                .unwrap()
                .len(),
            size.div_ceil(3) * 4
        );
        assert!(frame.to_string().len() > 4096);
        assert!(frame.to_string().len() < MAX_IMAGE_FRAME_BYTES);
    }
    let text = generic_tool_result(
        json!(1),
        crate::generic_mcp::Response {
            value: json!({"text":"x".repeat(200*1024)}),
            failed: false,
            image: None,
        },
        4096,
    );
    assert_eq!(
        text["result"]["structuredContent"]["code"],
        "response_too_large"
    );
}
