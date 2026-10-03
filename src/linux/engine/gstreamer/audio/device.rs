use crate::{
    common::gst::{
        GStreamer, app::DEFAULT_APPSRC_DO_TIMESTAMP, caps::GStreamerCapsType,
        element::GStreamerElementType, pipeline::GStreamerPipeline,
    },
    linux::engine::{
        gstreamer::audio::{DEFAULT_AUDIO_CHANNELS, DefaultDeviceType},
        pipewire::manager::{PipewireManager, PipewireNodeType},
    },
};
use gstreamer::{
    Element,
    glib::object::ObjectExt,
    prelude::{ElementExt, ElementExtManual, PadExt},
};
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

pub struct AudioDeviceFactory;

impl AudioDeviceFactory {
    /// This method will initialise all the audio-related things...
    /// This method will query the current pipewire state (which by the time this method is called
    /// should have already collected enough data).
    /// We will try to find the specified node names in current pipewire state. If fail -> display
    /// error & use default devices.
    /// This method is not responsible for CHANING user settings if something is wrong. Only using
    /// defaults to avoid fatal errors.
    pub fn setup_devices(
        pipeline: &GStreamerPipeline,
        user_settings: &UserSettings,
        mixer: &Element,
        manager: &PipewireManager,
    ) -> Result<(), WayclipError> {
        let state = manager.current_state();
        let audio = &user_settings.recording.audio;

        if audio.background.enabled {
            let (node_name, node_level) = match manager
                // TODO: Fix many enums for Sink/Source/Microphone/Background/Unknown
                .is_node_name_valid(&audio.background.node_name, PipewireNodeType::Sink)
            {
                true => (&audio.background.node_name, audio.background.level.0),
                false => {
                    log::error!(
                        "Audio device {} doesnt exist. Falling back to system defaults.",
                        audio.background.node_name
                    );
                    (
                        &state
                            .default_sink
                            .ok_or_else(|| WayclipError::NotFound("No default sink found".into()))?
                            .node_name,
                        audio.background.level.0,
                    )
                }
            };

            let node_id = manager
                .get_node_id_from_node_name(node_name)
                .ok_or_else(|| WayclipError::NotFound("No ID found for the sink".into()))?;

            Self::setup_audio_device(
                pipeline,
                user_settings,
                node_id,
                node_level,
                DefaultDeviceType::Background,
                mixer,
            )?;
        }

        if audio.microphone.enabled {
            let (node_name, node_level) = match manager
                .is_node_name_valid(&audio.microphone.node_name, PipewireNodeType::Source)
            {
                true => (&audio.microphone.node_name, audio.microphone.level.0),
                false => {
                    log::error!(
                        "Audio device {} doesnt exist. Falling back to system defaults.",
                        audio.microphone.node_name
                    );
                    (
                        &state
                            .default_source
                            .ok_or_else(|| {
                                WayclipError::NotFound("No default source found".into())
                            })?
                            .node_name,
                        1.0,
                    )
                }
            };

            let node_id = manager
                .get_node_id_from_node_name(node_name)
                .ok_or_else(|| WayclipError::NotFound("No ID found for the source".into()))?;

            Self::setup_audio_device(
                pipeline,
                user_settings,
                node_id,
                node_level,
                DefaultDeviceType::Microphone,
                mixer,
            )?;
        }

        Ok(())
    }

    /// This method will solely use minimal information provided to link up the correct audio device
    /// to our pipeline.
    ///
    /// No safety checks are made directly here if the node is on or if its valid, since that is
    /// done before calling this method
    fn setup_audio_device(
        pipeline: &GStreamerPipeline,
        user_settings: &UserSettings,
        audio_node_id: u32,
        audio_node_level: f64,
        audio_node_type: DefaultDeviceType,
        mixer: &Element,
    ) -> Result<(), WayclipError> {
        let pipewire_src = GStreamer::build_element(GStreamerElementType::AudioPipewireSrc {
            do_timestamp: DEFAULT_APPSRC_DO_TIMESTAMP,
            target_object: audio_node_id.to_string().into(),
            sink: audio_node_type.is_sink(),
        })?;

        let queue = GStreamer::build_element(GStreamerElementType::AudioQueue)?;

        let caps = GStreamer::build_caps(GStreamerCapsType::AudioXRaw {
            rate: Some(user_settings.recording.audio.sample_rate_hz.0 as i32),
            channels: DEFAULT_AUDIO_CHANNELS,
        });
        let caps_filter = GStreamer::build_element(GStreamerElementType::CapsFilter { caps })?;

        let audioconvert = GStreamer::build_element(GStreamerElementType::AudioConvert)?;
        let audioresample = GStreamer::build_element(GStreamerElementType::AudioResample)?;

        let sink_pad = mixer
            .request_pad_simple("sink_%u")
            .ok_or_else(|| WayclipError::NotFound("No sink pad found".into()))?;
        sink_pad.set_property("volume", audio_node_level);

        pipeline.add_and_link(&[
            &pipewire_src,
            &queue,
            &audioconvert,
            &audioresample,
            &caps_filter,
        ])?;

        let src_pad = caps_filter
            .static_pad("src")
            .ok_or_else(|| WayclipError::NotFound("No src pad from capsfilter".into()))?;
        src_pad.link(&sink_pad)?;

        Ok(())
    }
}
