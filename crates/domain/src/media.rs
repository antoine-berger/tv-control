//! Bounded media discovery. Provider identifiers are data, never executable URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read {
    Search {
        query: String,
        limit: u16,
    },
    Item {
        item_id: String,
    },
    Episodes {
        series_id: String,
        season: Option<u32>,
        limit: u16,
    },
    Playback,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub device: String,
    pub expires_at_unix_ms: u64,
    pub read: Read,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub series_id: Option<String>,
    pub series_name: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub duration_ticks: Option<u64>,
    pub sources: Vec<Source>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub id: String,
    pub duration_ticks: Option<u64>,
    pub tracks: Vec<Track>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub index: u32,
    pub kind: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playback {
    pub item_id: Option<String>,
    pub source_id: Option<String>,
    pub position_ticks: Option<u64>,
    pub paused: Option<bool>,
    pub can_seek: Option<bool>,
    pub supports_media_control: Option<bool>,
    pub audio_index: Option<i32>,
    pub subtitle_index: Option<i32>,
    pub client: Option<String>,
    pub client_version: Option<String>,
    pub supported_commands: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Data {
    Items { items: Vec<Item>, truncated: bool },
    Playback(Playback),
}
