//! Wizard progress only: restored choices never authorize network, TV or tunnel access.
use super::*;
use tv_adapters::setup_checkpoint::{Checkpoint, Store};
pub(super) struct Progress {
    store: Store,
    state: Checkpoint,
}
impl Progress {
    pub fn open(options: &Options) -> Result<Option<Self>, &'static str> {
        if options.dry
            || options.no_checkpoint
            || (options.json && options.get("--checkpoint").is_none())
        {
            return Ok(None);
        }
        let output = PathBuf::from(options.get("--output").unwrap_or("tv-control.local.json"));
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()
            .map_err(|_| "output_directory_missing")?;
        let output = parent.join(output.file_name().ok_or("invalid_output_path")?);
        let path = options
            .get("--checkpoint")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                output.with_file_name(format!(
                    "{}.setup-state.json",
                    output.file_name().unwrap().to_string_lossy()
                ))
            });
        let checkpoint_parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()
            .map_err(|_| "checkpoint_directory_missing")?;
        let path = checkpoint_parent.join(path.file_name().ok_or("checkpoint_invalid_path")?);
        if path == output {
            return Err("checkpoint_cannot_replace_configuration");
        }
        let mut store = Store::open(&path)?;
        let loaded = store.load();
        let loaded = if loaded
            .as_ref()
            .is_err_and(|e| *e == "checkpoint_multiple_links")
            && !options.json
        {
            if prompt("Setup stopped during checkpoint publication. Keep its progress and finish recovery? Type yes:")?!="yes" {return Err("checkpoint_recovery_cancelled");}
            store.recover_publication()?;
            store.load()
        } else {
            loaded
        };
        let saved = match loaded {
            Ok(saved) => saved,
            Err(error)
                if !options.json
                    && matches!(
                        error,
                        "checkpoint_corrupt_use_new_path"
                            | "checkpoint_invalid"
                            | "checkpoint_too_large"
                    ) =>
            {
                if prompt("Saved setup progress is unreadable. Keep a private backup and start setup again? Type yes:")?!="yes" {return Err("checkpoint_recovery_cancelled");}
                store.quarantine()?;
                None
            }
            Err(error) => return Err(error),
        };
        let state = saved.unwrap_or_else(|| Checkpoint {
            schema: 1,
            writer_version: env!("CARGO_PKG_VERSION").into(),
            output: output.clone(),
            ..Checkpoint::default()
        });
        if state.output != output {
            return Err("checkpoint_output_mismatch");
        }
        Ok(Some(Self { store, state }))
    }
    pub fn restore(
        &mut self,
        options: &mut Options,
        port: &dyn NetworkDiscovery,
    ) -> Result<bool, &'static str> {
        if self.state.interface.is_none() {
            return Ok(false);
        }
        if options.json {
            return Err("checkpoint_resume_requires_explicit_selection");
        }
        eprintln!(
            "Resuming local setup. Network, TV authorization and ChatGPT access remain unverified."
        );
        let all = port.interfaces().map_err(fail)?;
        if !all.iter().any(|i| {
            self.state
                .interface
                .as_ref()
                .is_some_and(|s| s == &format!("{}@{}", i.name, i.address))
        }) {
            eprintln!("The saved interface/address changed. Choose the LAN again.");
            self.state.interface = None;
            self.state.device = None;
            self.state.manual_endpoint = None;
            self.state.manual_kind = None;
            self.save()?;
            return Ok(false);
        }
        for (key, value) in [
            ("--interface", self.state.interface.clone()),
            ("--device", self.state.device.clone()),
            (
                "--adb",
                self.state
                    .adb
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
            ),
            (
                "--curl",
                self.state
                    .curl
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
            ),
            (
                "--server-port",
                self.state.server_port.map(|p| p.to_string()),
            ),
        ] {
            if let Some(value) = value {
                options.values.entry(key.into()).or_insert(value);
            }
        }
        if options.get("--manual-endpoint").is_none() {
            if let (Some(endpoint), Some(kind)) =
                (&self.state.manual_endpoint, &self.state.manual_kind)
            {
                if prompt(&format!("Is the saved TV connection endpoint {endpoint} ({kind}) still current? Type yes to reuse, or Enter to choose again:"))?=="yes" {
                    options.values.insert("--manual-endpoint".into(),endpoint.clone());options.values.insert("--manual-kind".into(),kind.clone());
                }
            }
        }
        Ok(true)
    }
    pub fn interface(
        &mut self,
        interface: &str,
        port: &dyn NetworkDiscovery,
    ) -> Result<(), &'static str> {
        let all = port.interfaces().map_err(fail)?;
        let matches: Vec<_> = all
            .iter()
            .filter(|i| i.name == interface || format!("{}@{}", i.name, i.address) == interface)
            .collect();
        if matches.len() != 1 {
            return Err("interface_missing_or_ambiguous");
        }
        self.state.interface = Some(format!("{}@{}", matches[0].name, matches[0].address));
        self.save()
    }
    pub fn target(&mut self, plan: &SetupPlan, manual: bool) -> Result<(), &'static str> {
        self.state.device = Some(plan.candidate.id.clone());
        if self.state.device.as_ref().is_some_and(|s| s.is_empty()) {
            self.state.device = None;
        }
        self.state.manual_endpoint = if manual { plan.endpoint.clone() } else { None };
        self.state.manual_kind = if manual {
            Some(
                if plan
                    .candidate
                    .services
                    .iter()
                    .any(|s| s.kind == ServiceKind::AdbTlsConnect)
                {
                    "tls"
                } else {
                    "legacy"
                }
                .into(),
            )
        } else {
            None
        };
        self.save()
    }
    pub fn dependencies(&mut self, config: &Config) -> Result<(), &'static str> {
        self.state.adb = Some(config.executables.adb.clone());
        self.state.curl = Some(config.executables.curl.clone());
        if let Some(tv_adapters::config::Device::Adb { server_port, .. }) =
            config.devices.values().next()
        {
            self.state.server_port = Some(*server_port);
        }
        self.save()
    }
    pub fn completed(&mut self) -> Result<(), &'static str> {
        self.state.configuration_saved = true;
        self.save()
    }
    pub fn clear_target(&mut self) -> Result<(), &'static str> {
        self.state.device = None;
        self.state.manual_endpoint = None;
        self.state.manual_kind = None;
        self.save()
    }
    fn save(&mut self) -> Result<(), &'static str> {
        self.state.writer_version = env!("CARGO_PKG_VERSION").into();
        self.store.save(&self.state)
    }
}
