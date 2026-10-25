//! Installation-scoped update guard, independent of ambient config-directory selection.
use super::*;

pub(super) fn path(binding: &Binding) -> PortResult<PathBuf> {
    Ok(binding
        .launcher
        .parent()
        .ok_or(Failure::before("installation_path_invalid"))?
        .join(".tv-control-supervision"))
}
fn bytes(binding: &Binding) -> PortResult<Vec<u8>> {
    serde_json::to_vec(
        &json!({"schema":1,"installation_id":binding.installation_id,
        "configuration":binding.configuration,"label":binding.label}),
    )
    .map_err(|_| Failure::before("service_marker_invalid"))
}
pub(super) fn create(binding: &Binding) -> PortResult<()> {
    artifact::create(&path(binding)?, &bytes(binding)?)
}
pub(super) fn verify(binding: &Binding) -> PortResult<()> {
    artifact::verify(&path(binding)?, &bytes(binding)?)
}
pub(super) fn remove(binding: &Binding) -> PortResult<()> {
    artifact::remove(&path(binding)?, &bytes(binding)?)
}
