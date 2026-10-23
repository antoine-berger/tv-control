use std::path::{Path, PathBuf};

pub const VERSION: &str = "v0.0.17";
const ROOT: &str = "https://github.com/openai/tunnel-client/releases/download";
const OWNER: &str = "TV_CONTROL_TUNNEL_DEPENDENCY_V1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Package {
    Tunnel,
    Adb,
}

#[derive(Clone, Copy, Debug)]
pub struct Release {
    pub(super) package: Package,
    pub(super) platform: &'static str,
    pub(super) sha256: &'static str,
}
impl Release {
    pub fn linux(arch: &str) -> Result<Self, &'static str> {
        match arch {
            "x86_64" => Ok(Self {
                package: Package::Tunnel,
                platform: "linux-amd64",
                sha256: "bd6a57c6c0511644267cafadd13ffdb53ece0d753e0b88857f957e78a8fbc793",
            }),
            "aarch64" => Ok(Self {
                package: Package::Tunnel,
                platform: "linux-arm64",
                sha256: "f5060d0b63663268aab3ca310cb6110fa14867981db2b0657f679d8836a2daa0",
            }),
            _ => Err("tunnel_dependency_platform_unsupported"),
        }
    }
    pub fn adb_linux(arch: &str) -> Result<Self, &'static str> {
        if arch != "x86_64" {
            return Err("adb_official_archive_platform_unsupported");
        }
        Ok(Self {
            package: Package::Adb,
            platform: "linux-amd64",
            sha256: "d230f13842f60f782a8645f9c813f8f845bf36089ea7289f28c48f17979313f1",
        })
    }
    pub(super) fn binary(self) -> &'static str {
        match self.package {
            Package::Tunnel => "tunnel-client",
            Package::Adb => "adb",
        }
    }
    pub(super) fn owner(self) -> &'static str {
        match self.package {
            Package::Tunnel => OWNER,
            Package::Adb => "TV_CONTROL_ADB_DEPENDENCY_V1",
        }
    }
    pub(super) fn url(self) -> String {
        if self.package == Package::Adb {
            return format!("https://dl.google.com/android/repository/{}", self.asset());
        }
        format!("{ROOT}/{VERSION}/{}", self.asset())
    }
    pub(super) fn payload(self) -> Vec<(String, String, bool)> {
        if self.package == Package::Adb {
            return ["adb", "NOTICE.txt", "source.properties"]
                .into_iter()
                .map(|name| (format!("platform-tools/{name}"), name.into(), name == "adb"))
                .collect();
        }
        self.members()
            .into_iter()
            .map(|name| {
                let executable = name == "tunnel-client" || name == "cloudflared";
                (name.clone(), name, executable)
            })
            .collect()
    }
    pub(super) fn asset(self) -> String {
        if self.package == Package::Adb {
            return "platform-tools_r37.0.1-linux.zip".into();
        }

        format!("tunnel-client-{VERSION}-{}.zip", self.platform)
    }
    pub(super) fn members(self) -> Vec<String> {
        if self.package == Package::Adb {
            return [
                "NOTICE.txt",
                "adb",
                "etc1tool",
                "fastboot",
                "hprof-conv",
                "lib64/libc++.so",
                "make_f2fs",
                "make_f2fs_casefold",
                "mke2fs",
                "mke2fs.conf",
                "source.properties",
                "sqlite3",
            ]
            .into_iter()
            .map(|name| format!("platform-tools/{name}"))
            .collect();
        }

        vec![
            "tunnel-client".into(),
            "cloudflared".into(),
            "cloudflared-manifest.json".into(),
            "LICENSE".into(),
            "NOTICE".into(),
            format!("tunnel-client-{VERSION}-{}-licenses.txt", self.platform),
            format!("tunnel-client-{VERSION}-{}.spdx.json", self.platform),
        ]
    }
    pub fn binary_path(self, state: &Path) -> PathBuf {
        self.directory(state).join(self.binary())
    }
    pub(super) fn directory(self, state: &Path) -> PathBuf {
        if self.package == Package::Adb {
            return state.join("adb-37.0.1-linux-amd64");
        }

        state.join(format!("tunnel-client-{VERSION}-{}", self.platform))
    }
}
pub(super) fn executable_header(bytes: &[u8], release: Release) -> Result<(), &'static str> {
    if bytes.len() < 20
        || bytes[..6] != *b"\x7fELF\x02\x01"
        || !matches!(&bytes[16..18], [2, 0] | [3, 0])
    {
        return Err("tunnel_dependency_binary_format_mismatch");
    }
    let machine = if release.platform == "linux-amd64" {
        [62, 0]
    } else {
        [183, 0]
    };
    if bytes[18..20] != machine {
        return Err("tunnel_dependency_architecture_mismatch");
    }
    Ok(())
}
