pub mod adb;
pub mod adb_server;
pub mod config;
pub mod http;
pub mod jellyfin;
pub mod process;
pub mod runtime;
pub mod stdio;
pub mod wire;

use config::{Config, Device};
use std::sync::Arc;
use tv_application::{Context, DeviceControl, MediaCatalogue};
use tv_domain::*;

#[derive(Clone)]
pub struct Router {
    pub config: Arc<Config>,
    pub process: Arc<dyn process::Process>,
    pub http: Arc<dyn http::Http>,
    pub adb_probe: Arc<dyn adb_server::ServerProbe>,
}
impl DeviceControl for Router {
    fn observe(&self, alias: &str, ctx: &Context<'_>) -> PortResult<Observed> {
        match self
            .config
            .devices
            .get(alias)
            .ok_or(Failure::before("unknown_device"))?
        {
            Device::Adb { .. } => {
                adb::Adb::new(&self.config, &*self.process, &*self.adb_probe).observe(alias, ctx)
            }
            Device::Jellyfin { .. } => {
                jellyfin::Jellyfin::new(&self.config, &*self.http).observe(alias, ctx)
            }
        }
    }
    fn apply(
        &self,
        alias: &str,
        action: &Action,
        plan: Option<&MediaPlan>,
        ctx: &Context<'_>,
    ) -> PortResult<()> {
        match self
            .config
            .devices
            .get(alias)
            .ok_or(Failure::before("unknown_device"))?
        {
            Device::Adb { .. } => adb::Adb::new(&self.config, &*self.process, &*self.adb_probe)
                .apply(alias, action, ctx),
            Device::Jellyfin { .. } => {
                jellyfin::Jellyfin::new(&self.config, &*self.http).apply(alias, action, plan, ctx)
            }
        }
    }
}
impl MediaCatalogue for Router {
    fn resolve(&self, device: &str, alias: &str, ctx: &Context<'_>) -> PortResult<MediaPlan> {
        jellyfin::Jellyfin::new(&self.config, &*self.http).resolve(device, alias, ctx)
    }
}
pub mod discovery;
