use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::IpAddr, path::PathBuf};
use tv_domain::identifier;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub preferences: Preferences,
    pub executables: Executables,
    #[serde(deserialize_with = "unique_map")]
    pub devices: BTreeMap<String, Device>,
    #[serde(deserialize_with = "unique_map")]
    pub apps: BTreeMap<String, App>,
    #[serde(deserialize_with = "unique_map")]
    pub servers: BTreeMap<String, Server>,
    #[serde(deserialize_with = "unique_map")]
    pub media: BTreeMap<String, Media>,
    #[serde(deserialize_with = "unique_map")]
    pub workflows: BTreeMap<String, Workflow>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub max_budget_ms: u64,
    pub verification_poll_ms: u64,
    pub process_poll_ms: u64,
    pub max_output_bytes: usize,
    pub max_frame_bytes: usize,
    pub max_inflight: usize,
    pub dedup_entries: usize,
    pub dedup_retention_ms: u64,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Executables {
    pub adb: PathBuf,
    pub curl: PathBuf,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "adapter", rename_all = "snake_case", deny_unknown_fields)]
pub enum Device {
    Adb {
        endpoint: String,
        server_host: String,
        server_port: u16,
        media_app: Option<String>,
        volume_max_index: Option<u16>,
    },
    Jellyfin {
        server: String,
        user_id: String,
        device_id: String,
    },
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    pub package: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub base_url: String,
    pub token_env: String,
    pub allow_insecure_http: bool,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Media {
    pub server: String,
    pub series_id: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub device: String,
    pub media: String,
}

impl Config {
    pub fn from_json(bytes: &[u8]) -> Result<Self, &'static str> {
        let cfg: Self = serde_json::from_slice(bytes).map_err(|_| "invalid_configuration_json")?;
        cfg.validate()?;
        Ok(cfg)
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        let p = &self.preferences;
        if self.version != 1
            || !(1..=10_000).contains(&p.max_budget_ms)
            || !(1..=500).contains(&p.verification_poll_ms)
            || !(1..=50).contains(&p.process_poll_ms)
            || !(1024..=1_048_576).contains(&p.max_output_bytes)
            || !(1024..=65536).contains(&p.max_frame_bytes)
            || !(1..=8).contains(&p.max_inflight)
            || !(1..=4096).contains(&p.dedup_entries)
            || p.dedup_retention_ms < p.max_budget_ms
            || p.dedup_retention_ms > 86_400_000
        {
            return Err("invalid_resource_policy");
        }
        if !self.executables.adb.is_absolute() || !self.executables.curl.is_absolute() {
            return Err("executable_must_be_absolute");
        }
        if self.devices.is_empty()
            || self.devices.len() > 64
            || self.apps.len() > 64
            || self.servers.len() > 16
            || self.media.len() > 64
            || self.workflows.len() > 64
        {
            return Err("invalid_device_count");
        }
        for key in self
            .devices
            .keys()
            .chain(self.apps.keys())
            .chain(self.servers.keys())
            .chain(self.media.keys())
            .chain(self.workflows.keys())
        {
            if !identifier(key) {
                return Err("invalid_alias");
            }
        }
        let mut packages = std::collections::BTreeSet::new();
        for app in self.apps.values() {
            if !package(&app.package) {
                return Err("invalid_package");
            }
            if !packages.insert(&app.package) {
                return Err("duplicate_app_package");
            }
        }
        for server in self.servers.values() {
            validate_url(&server.base_url, server.allow_insecure_http)?;
            if !environment_name(&server.token_env) {
                return Err("invalid_secret_reference");
            }
        }
        for device in self.devices.values() {
            match device {
                Device::Adb {
                    endpoint,
                    server_host,
                    server_port,
                    media_app,
                    volume_max_index,
                } => {
                    let addr: std::net::SocketAddr =
                        endpoint.parse().map_err(|_| "invalid_adb_endpoint")?;
                    if addr.port() == 0 || addr.ip().is_unspecified() || addr.ip().is_multicast() {
                        return Err("invalid_adb_endpoint");
                    }
                    let host: IpAddr = server_host
                        .parse()
                        .map_err(|_| "adb_server_must_be_loopback")?;
                    if !host.is_ipv4() || !host.is_loopback() || *server_port == 0 {
                        return Err("adb_server_must_be_loopback");
                    }
                    if media_app
                        .as_ref()
                        .is_some_and(|a| !self.apps.contains_key(a))
                    {
                        return Err("unknown_media_app");
                    }
                    if volume_max_index.is_some_and(|m| m == 0 || m > 1000) {
                        return Err("invalid_volume_scale");
                    }
                }
                Device::Jellyfin {
                    server,
                    user_id,
                    device_id,
                } => {
                    if !self.servers.contains_key(server)
                        || !opaque_id(user_id)
                        || !opaque_id(device_id)
                    {
                        return Err("invalid_jellyfin_device");
                    }
                }
            }
        }
        for media in self.media.values() {
            if !self.servers.contains_key(&media.server) || !opaque_id(&media.series_id) {
                return Err("invalid_media");
            }
        }
        for workflow in self.workflows.values() {
            if !self.devices.contains_key(&workflow.device)
                || !self.media.contains_key(&workflow.media)
            {
                return Err("invalid_workflow");
            }
            match &self.devices[&workflow.device] {
                Device::Jellyfin { server, .. }
                    if *server == self.media[&workflow.media].server => {}
                _ => return Err("workflow_backend_mismatch"),
            }
        }
        Ok(())
    }
}
pub fn package(s: &str) -> bool {
    s.len() <= 200
        && s.contains('.')
        && s.split('.').all(|part| {
            !part.is_empty()
                && part.as_bytes()[0].is_ascii_alphabetic()
                && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        })
}
pub fn opaque_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn environment_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
}
pub fn validate_url(s: &str, allow_http: bool) -> Result<(), &'static str> {
    if s.len() > 512
        || !s.is_ascii()
        || s.contains(['\r', '\n', '"', '\\', '@', '?', '#', '{', '}'])
        || s.chars().any(char::is_whitespace)
    {
        return Err("invalid_server_url");
    }
    let authority = s
        .strip_prefix("https://")
        .or_else(|| {
            if allow_http {
                s.strip_prefix("http://")
            } else {
                None
            }
        })
        .ok_or("tls_required")?;
    if authority.is_empty() || authority.starts_with('/') || authority.ends_with('/') {
        return Err("invalid_server_url");
    }
    let host = authority.split('/').next().ok_or("invalid_server_url")?;
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
    {
        return Err("invalid_server_url");
    }
    Ok(())
}

fn unique_map<'de, D, T>(deserializer: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Unique<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Unique<T> {
        type Value = BTreeMap<String, T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a map with unique aliases")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, T>()? {
                if result.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate alias"));
                }
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(Unique(std::marker::PhantomData))
}
