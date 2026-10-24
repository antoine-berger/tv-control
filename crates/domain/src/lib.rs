//! Domain vocabulary. No transport, filesystem, networking, or serialization dependencies.
pub mod pipeline;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Observe,
    Pause,
    Resume,
    OpenApp {
        app: String,
    },
    SetVolume {
        index: u16,
    },
    ResumeMedia {
        media: String,
    },
    PlayItem {
        item_id: String,
        source_id: String,
        position_ms: u64,
    },
    SeekMedia {
        item_id: String,
        source_id: String,
        position_ms: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: String,
    pub device: String,
    pub action: Action,
    /// Absolute expiry supplied by the calling client, never extended on receipt.
    pub expires_at_unix_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Playback {
    Unknown,
    Stopped,
    Paused,
    Playing,
    Buffering,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    /// Native power observation; None means unavailable, not asleep.
    pub awake: Option<bool>,
    pub connected: bool,
    pub active_app: Option<String>,
    pub playback: Playback,
    pub playback_evidence: PlaybackEvidence,
    pub media_id: Option<String>,
    pub media_source_id: Option<String>,
    pub position_ticks: Option<u64>,
    pub volume_index: Option<u16>,
    pub volume_max_index: Option<u16>,
}
impl Default for Observed {
    fn default() -> Self {
        Self {
            awake: None,
            connected: false,
            active_app: None,
            playback: Playback::Unknown,
            playback_evidence: PlaybackEvidence::PositionProgressRequired,
            media_id: None,
            media_source_id: None,
            position_ticks: None,
            volume_index: None,
            volume_max_index: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackEvidence {
    NativeState,
    PositionProgressRequired,
}
/// Jellyfin media positions use ticks of 100 ns.
pub const MEDIA_TICKS_PER_MILLISECOND: u64 = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaPlan {
    pub item_id: String,
    pub source_id: Option<String>,
    pub media_kind: Option<String>,
    pub position_ticks: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Observed,
    Verified,
    Invalid,
    Busy,
    Unsupported,
    Expired,
    Cancelled,
    Failed,
    OutcomeUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub request_id: String,
    pub status: Status,
    pub code: &'static str,
    pub observed: Option<Observed>,
    /// Controller-local runtime. This is explicitly not user-to-TV latency.
    pub local_elapsed_ms: u64,
    pub cached: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    MayHaveChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub effect: Effect,
    /// Optional numeric operating-system evidence; never a raw message or credential.
    pub os_error: Option<i32>,
}
impl Failure {
    pub const fn before(code: &'static str) -> Self {
        Self {
            code,
            effect: Effect::None,
            os_error: None,
        }
    }
    pub const fn uncertain(code: &'static str) -> Self {
        Self {
            code,
            effect: Effect::MayHaveChanged,
            os_error: None,
        }
    }
    pub const fn with_os_error(mut self, code: Option<i32>) -> Self {
        self.os_error = code;
        self
    }
}
pub type PortResult<T> = Result<T, Failure>;

pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

pub mod onboarding;

pub mod generic;

pub mod media;
