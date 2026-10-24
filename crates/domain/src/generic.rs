//! Platform-neutral, bounded discovery and intent vocabulary.
use crate::{Observed, Status};

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
    pub bounds: [u32; 4],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiSnapshot {
    pub revision: String,
    pub elements: Vec<UiElement>,
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
