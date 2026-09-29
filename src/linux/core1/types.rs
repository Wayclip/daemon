use serde::{Deserialize, Serialize};
use wayclip_core::settings::recording::{AudioSettings, Bitrate, Fps, Resolution, VideoCodec};
use zbus::zvariant::Type;

#[derive(Clone, Debug)]
pub struct RecordingConfig {
    pub codec: VideoCodec,
    pub bitrate_kbps: Bitrate,
    pub resolution: Resolution,
    pub fps: Fps,
    pub audio: AudioSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Type, Serialize, Deserialize)]
pub enum DaemonStatus {
    Active,
    Inactive,
    Saving,
    Activating,
    Deactivating,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefaultDeviceType {
    Microphone,
    Background,
}
