//! Offline build evidence, never a liveness or authorization claim.
use super::runtime::{private_read_limit, RuntimeIntent};
use serde_json::{json, Value};
use std::path::Path;

pub fn runtime_diagnostics(path: &Path, snapshot: &[u8]) -> Value {
    let result = last_started(path, snapshot);
    let (version, status) = match result {
        Ok(version) => (Some(version), "recorded"),
        Err(code) => (None, code),
    };
    json!({
        "invoked_version": env!("CARGO_PKG_VERSION"),
        "last_started_version": version,
        "last_started_matches_invoked": version.as_deref().map(|v| v == env!("CARGO_PKG_VERSION")),
        "record_status": status,
        "running_version": null,
        "running_verified": false,
        "note": "The invoked CLI version and last-started bridge are separate. Offline records do not prove the bridge is running. A live MCP initialize response reports that server's version."
    })
}

fn last_started(path: &Path, snapshot: &[u8]) -> Result<String, &'static str> {
    let path = path
        .canonicalize()
        .map_err(|_| "configuration_path_invalid")?;
    let state = path
        .parent()
        .ok_or("configuration_path_invalid")?
        .join("connection");
    // Do not open a Store here: doctor must never create directories, take over
    // a session, recover temporary files or rewrite any connection evidence.
    let intent: RuntimeIntent = serde_json::from_slice(&private_read_limit(
        &state.join("runtime-intent.json"),
        6_400_000,
    )?)
    .map_err(|_| "runtime_intent_invalid")?;
    if intent.schema != 1
        || !tv_domain::identifier(&intent.installation_id)
        || !tv_domain::identifier(&intent.revision)
        || !tv_domain::identifier(&intent.device)
    {
        return Err("runtime_intent_invalid");
    }
    if intent.configuration.as_bytes() != snapshot {
        return Err("runtime_configuration_changed");
    }
    tv_adapters::runtime_intent::last_started_version(
        &state,
        &intent.installation_id,
        &intent.revision,
        &intent.device,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    struct Temporary(PathBuf);
    impl Temporary {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "doctor-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
        fn config(&self) -> PathBuf {
            let path = self.0.join("config.json");
            fs::write(&path, b"fixture configuration").unwrap();
            path
        }
        fn prepare(&self) -> PathBuf {
            let state = self.0.join("connection");
            fs::create_dir(&state).unwrap();
            fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
            let intent = RuntimeIntent {
                schema: 1,
                installation_id: "installation".into(),
                revision: "revision".into(),
                device: "screen".into(),
                configuration: "fixture configuration".into(),
                controller_binary: "/example/controller".into(),
            };
            let path = state.join("runtime-intent.json");
            fs::write(&path, serde_json::to_vec(&intent).unwrap()).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
            state
        }
    }
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn missing_runtime_is_unknown_without_creating_any_state() {
        let temp = Temporary::new();
        let path = temp.config();
        let result = runtime_diagnostics(&path, b"fixture configuration");
        assert_eq!(result["invoked_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(result["running_verified"], false);
        assert!(result["running_version"].is_null());
        assert!(result["last_started_version"].is_null());
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
    }
    #[test]
    fn historical_version_never_claims_liveness_and_changed_configuration_is_rejected() {
        let temp = Temporary::new();
        let path = temp.config();
        let state = temp.prepare();
        let generation = tv_adapters::runtime_intent::start_generation(
            &state,
            "installation",
            "revision",
            "screen",
        )
        .unwrap();
        tv_adapters::runtime_intent::record_build(&state, &generation, "0.0.1").unwrap();
        let before = fs::read(state.join("bridge-build.json")).unwrap();
        let result = runtime_diagnostics(&path, b"fixture configuration");
        assert_eq!(result["last_started_version"], "0.0.1");
        assert_eq!(result["last_started_matches_invoked"], false);
        assert_eq!(result["record_status"], "recorded");
        assert_eq!(result["running_verified"], false);
        assert!(result["running_version"].is_null());
        let changed = runtime_diagnostics(&path, b"new configuration");
        assert_eq!(changed["record_status"], "runtime_configuration_changed");
        assert!(changed["last_started_version"].is_null());
        assert_eq!(fs::read(state.join("bridge-build.json")).unwrap(), before);
    }
}
