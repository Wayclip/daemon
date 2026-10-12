use crate::{
    common::gst::{
        GStreamer, app::DEFAULT_APPSRC_DO_TIMESTAMP, caps::GStreamerCapsType,
        element::GStreamerElementType, pipeline::GStreamerPipeline,
    },
    linux::engine::gstreamer::audio::DEFAULT_AUDIO_CHANNELS,
};
use gstreamer::{
    Element,
    glib::object::ObjectExt,
    prelude::{ElementExt, ElementExtManual, PadExt},
};
use wayclip_core::{
    models::{
        daemon::pipewire::{PipewireManager, PipewireNodeType},
        error::WayclipError,
    },
    settings::UserSettings,
};

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
        let audio = &user_settings.recording.audio;

        if audio.background.enabled {
            if let Err(e) = Self::attach(
                pipeline,
                user_settings,
                &audio.background.node_name,
                PipewireNodeType::Sink,
                audio.background.level.0,
                mixer,
                manager,
            ) {
                log::error!("Desktop audio disabled: {e}");
            }
        }

        if audio.microphone.enabled {
            if let Err(e) = Self::attach(
                pipeline,
                user_settings,
                &audio.microphone.node_name,
                PipewireNodeType::Source,
                audio.microphone.level.0,
                mixer,
                manager,
            ) {
                log::error!("microphone audio disabled: {e}");
            }
        }

        Ok(())
    }

    fn attach(
        pipeline: &GStreamerPipeline,
        user_settings: &UserSettings,
        audio_node_name: &str,
        audio_node_type: PipewireNodeType,
        audio_node_level: f64,
        mixer: &Element,
        manager: &PipewireManager,
    ) -> Result<(), WayclipError> {
        let state = manager.current_state();
        let name = if manager.is_node_name_valid(audio_node_name, audio_node_type) {
            audio_node_name.to_string()
        } else {
            log::warn!(
                "Audio device {} not found, using system default",
                audio_node_name
            );

            let default = match audio_node_type {
                PipewireNodeType::Sink => state.default_sink,
                PipewireNodeType::Source => state.default_source,
            };

            default
                .ok_or_else(|| {
                    WayclipError::NotFound(format!("No default {:?} found", audio_node_type).into())
                })?
                .node_name
        };

        let id = manager
            .get_node_id_from_node_name(&name)
            .ok_or_else(|| WayclipError::NotFound("No ID found for the sink".into()))?;

        Self::setup_audio_device(
            pipeline,
            user_settings,
            id,
            audio_node_level,
            audio_node_type,
            mixer,
        )
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
        audio_node_type: PipewireNodeType,
        mixer: &Element,
    ) -> Result<(), WayclipError> {
        let pipewire_src = GStreamer::build_element(GStreamerElementType::AudioPipewireSrc {
            do_timestamp: DEFAULT_APPSRC_DO_TIMESTAMP,
            // Passing ID seems like the correct choice
            target_object: audio_node_id.to_string().into(),
            sink: audio_node_type == PipewireNodeType::Sink,
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
