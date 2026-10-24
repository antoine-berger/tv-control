//! Observed input inventory, not source selection or video evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputConnection {
    Connected,
    Standby,
    Disconnected,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TvInput {
    pub id: String,
    pub package: String,
    pub service: String,
    pub connection: InputConnection,
    /// Physical port only when a unique hardware mapping establishes it.
    pub hdmi_port: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputListing {
    pub current_user: u32,
    pub inputs: Vec<TvInput>,
    pub truncated: bool,
}
