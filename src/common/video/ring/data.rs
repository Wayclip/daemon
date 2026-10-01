use gstreamer::{Caps, ClockTime, StructureRef};
use strum_macros::Display;
use wayclip_core::models::error::WayclipError;

#[derive(Clone, Debug, Display, PartialEq, Eq)]
pub enum ContentType {
    #[strum(serialize = "video")]
    Video,
    #[strum(serialize = "audio")]
    Audio,
}

#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub payload: gstreamer::Buffer,
    pub pts: ClockTime,
    pub dts: Option<ClockTime>,
    pub duration: Option<ClockTime>,
    pub is_keyframe: bool,
}

impl EncodedFrame {
    pub fn new(
        payload: gstreamer::Buffer,
        pts: ClockTime,
        dts: Option<ClockTime>,
        duration: Option<ClockTime>,
        is_keyframe: bool,
    ) -> Self {
        Self {
            payload,
            pts,
            dts,
            duration,
            is_keyframe,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SaveData {
    pub video_frames: Vec<EncodedFrame>,
    pub audio_frames: Vec<EncodedFrame>,
    pub video_caps: gstreamer::Caps,
    pub audio_caps: Option<gstreamer::Caps>,
    // basically if video & audio frames are misaligned. this is offset of audio relative to video
    // (their first frames)
    pub sync_offset_ms: i64,
    pub duration: ClockTime,
}

impl SaveData {
    /// Get base pts for (video, audio)
    pub fn get_base_pts(&self) -> (ClockTime, ClockTime) {
        (
            self.video_frames
                .first()
                .map(|frame| frame.pts)
                .unwrap_or(gstreamer::ClockTime::ZERO),
            self.audio_frames
                .first()
                .map(|frame| frame.pts)
                .unwrap_or(gstreamer::ClockTime::ZERO),
        )
    }

    pub fn get_video_structure(&self) -> Result<&StructureRef, WayclipError> {
        self.video_caps
            .structure(0)
            .ok_or_else(|| WayclipError::Remux("Video caps has no structure".into()))
    }

    pub fn get_audio_structure(&self) -> Result<Option<(&Caps, &StructureRef)>, WayclipError> {
        match (&self.audio_caps, self.audio_frames.is_empty()) {
            (Some(caps), false) => Ok(Some((
                caps,
                caps.structure(0)
                    .ok_or_else(|| WayclipError::NotFound("No structure found".into()))?,
            ))),
            _ => Ok(None),
        }
    }

    pub fn clear_caps(&self, caps: Caps) -> Caps {
        let mut builder = Caps::builder_full();
        for structure in caps.iter() {
            builder = builder.structure(structure.to_owned());
        }
        builder.build()
    }

    pub fn get_bytes(&self) -> (u64, u64) {
        (
            self.video_frames
                .iter()
                .map(|f| f.payload.size() as u64)
                .sum::<u64>(),
            self.audio_frames
                .iter()
                .map(|f| f.payload.size() as u64)
                .sum::<u64>(),
        )
    }
}
