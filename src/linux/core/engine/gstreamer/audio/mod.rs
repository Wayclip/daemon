use gstreamer::Element;
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

use crate::common::gst::{
    GStreamer,
    caps::GStreamerCapsType,
    element::{GStreamerElement, GStreamerElementType},
};

pub mod device;

pub const DEFAULT_AUDIO_CHANNELS: i32 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefaultDeviceType {
    Microphone,
    Background,
}

impl DefaultDeviceType {
    pub fn is_sink(&self) -> bool {
        if self == &Self::Background {
            true
        } else {
            false
        }
    }
}

pub struct AudioBranchBuilder<'a> {
    user_settings: &'a UserSettings,
}

impl<'a> AudioBranchBuilder<'a> {
    pub fn new(user_settings: &'a UserSettings) -> Self {
        Self { user_settings }
    }

    pub fn build(self) -> Result<(Element, Vec<Element>), WayclipError> {
        let mix = GStreamer::build_element(GStreamerElementType::AudioMixer)?;

        let convert = GStreamer::build_element(GStreamerElementType::AudioConvert)?;

        let caps = GStreamer::build_caps(GStreamerCapsType::AudioXRaw {
            rate: None,
            channels: DEFAULT_AUDIO_CHANNELS,
        });
        let caps_filter = GStreamer::build_element(GStreamerElementType::CapsFilter { caps })?;

        let encoder = GStreamerElement::new(
            self.user_settings.recording.audio.codec.get_encoder(),
            vec![],
        )
        .build_element()?;

        let parser = GStreamerElement::new(
            self.user_settings.recording.audio.codec.get_parser(),
            vec![],
        )
        .build_element()?;

        let queue = GStreamer::build_element(GStreamerElementType::AudioQueue)?;

        Ok((
            mix.clone(),
            vec![mix, convert, caps_filter, encoder, parser, queue],
        ))
    }
}
