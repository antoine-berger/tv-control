//! Platform-neutral, bounded discovery and intent vocabulary.
use crate::{Observed, Playback, Status};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub id: String,
    pub component: String,
    /// Observed display name; None means the platform did not expose one.
    pub label: Option<String>,
    pub search_component: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
    Home,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    /// Internal identity for legacy actions sharing the same journal; never a public intent.
    Legacy {
        action: String,
    },
    OpenApp {
        app: String,
    },
    OpenUrl {
        app: String,
        url: String,
    },
    Wake,
    /// Set Android music stream to zero; never toggle mute or restore volume.
    Mute,
    SetVolume {
        index: u16,
    },
    Pause,
    Resume,
    Media {
        session: String,
        command: MediaCommand,
    },
    Navigate {
        keys: Vec<Key>,
    },
    Search {
        app: String,
        query: String,
    },
    Activate {
        reference: String,
    },
    /// Append printable ASCII to a freshly validated focused editable field.
    TypeText {
        reference: String,
        text: String,
    },
}
/// Session-scoped commands supported by the bounded Android monitor interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCommand {
    Pause,
    Resume,
    Next,
    Previous,
}
impl MediaCommand {
    pub fn name(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Next => "next",
            Self::Previous => "previous",
        }
    }
}
/// Untrusted native snapshot, not proof of audible output or content delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaSession {
    pub session: String,
    pub package: String,
    pub playback: Playback,
    /// Commands explicitly advertised by a scoped native action mask; None is unknown.
    pub supported_commands: Option<Vec<MediaCommand>>,
    /// Last application-reported position; not extrapolated to wall clock time.
    pub position_ms: Option<u64>,
    /// Native description, not a stable media identifier.
    pub description: Option<String>,
}
pub fn valid_media_session(value: &str) -> bool {
    let Some((package, tag)) = value.split_once('/') else {
        return false;
    };
    !package.is_empty()
        && package.len() <= 255
        && package
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.".contains(&b))
        && !tag.is_empty()
        && tag.len() <= 128
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: String,
    pub device: String,
    pub intent: Intent,
    pub expires_at_unix_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiElement {
    pub reference: String,
    pub text: String,
    pub role: String,
    pub focused: bool,
    pub actionable: bool,
    pub editable: bool,
    pub package: Option<String>,
    pub bounds: [u32; 4],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiSnapshot {
    pub revision: String,
    pub elements: Vec<UiElement>,
    pub truncation: Vec<UiTruncation>,
    /// Count all focused nodes, including omitted elements.
    pub focused_count: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiTruncation {
    Elements,
    Text,
}

/// Explicit last-resort image; never returned by ordinary state or control calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screenshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read {
    Media,
    State,
    Volume,
    Screenshot,
    Apps {
        query: Option<String>,
        since: Option<String>,
        limit: usize,
        refresh: bool,
    },
    Ui,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadRequest {
    pub device: String,
    pub read: Read,
    pub expires_at_unix_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Data {
    Media {
        sessions: Vec<MediaSession>,
        truncated: bool,
    },
    State(Observed),
    Screenshot(Screenshot),
    Apps {
        revision: String,
        unchanged: bool,
        apps: Vec<App>,
        truncated: bool,
    },
    Ui(UiSnapshot),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub request_id: String,
    pub status: Status,
    pub code: String,
    pub observed: Option<Observed>,
    pub choices: Vec<App>,
    pub cached: bool,
    pub local_elapsed_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Missing,
    Running,
    Complete(Reply),
}
