use serde::{Deserialize, Serialize};
use tv_domain::*;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WireRequest {
    pub request_id: String,
    pub device: String,
    pub action: WireAction,
    pub expires_at_unix_ms: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireAction {
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
impl From<WireRequest> for Request {
    fn from(r: WireRequest) -> Self {
        Self {
            id: r.request_id,
            device: r.device,
            expires_at_unix_ms: r.expires_at_unix_ms,
            action: match r.action {
                WireAction::Observe => Action::Observe,
                WireAction::Pause => Action::Pause,
                WireAction::Resume => Action::Resume,
                WireAction::OpenApp { app } => Action::OpenApp { app },
                WireAction::SetVolume { index } => Action::SetVolume { index },
                WireAction::ResumeMedia { media } => Action::ResumeMedia { media },
                WireAction::PlayItem {
                    item_id,
                    source_id,
                    position_ms,
                } => Action::PlayItem {
                    item_id,
                    source_id,
                    position_ms,
                },
                WireAction::SeekMedia {
                    item_id,
                    source_id,
                    position_ms,
                } => Action::SeekMedia {
                    item_id,
                    source_id,
                    position_ms,
                },
            },
        }
    }
}
pub fn receipt(r: &Receipt) -> serde_json::Value {
    use serde_json::json;
    json!({"request_id":r.request_id,"status":match r.status {
        Status::Observed=>"observed",Status::Verified=>"verified",Status::Invalid=>"invalid",Status::Busy=>"busy",Status::Unsupported=>"unsupported",
        Status::Expired=>"expired",Status::Cancelled=>"cancelled",Status::Failed=>"failed",Status::OutcomeUnknown=>"outcome_unknown"},
        "code":r.code,"cached":r.cached,"local_elapsed_ms":r.local_elapsed_ms,"timing_scope":"controller_local_not_user_to_tv",
        "observed":r.observed.as_ref().map(|s|json!({"connected":s.connected,"active_app":s.active_app,
            "playback_evidence":match s.playback_evidence {PlaybackEvidence::NativeState=>"native_state",PlaybackEvidence::PositionProgressRequired=>"position_progress_required"},"playback": match s.playback {
            Playback::Unknown=>"unknown",Playback::Stopped=>"stopped",Playback::Paused=>"paused",Playback::Playing=>"playing",Playback::Buffering=>"buffering"},
            "media_id":s.media_id,"media_source_id":s.media_source_id,"position_ticks":s.position_ticks,"volume_index":s.volume_index}))})
}
