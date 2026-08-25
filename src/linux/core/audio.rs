use crate::linux::core::types::DefaultDeviceType;
use crate::linux::core::{
    DEFAULT_AUDIO_CHANNELS, DEFAULT_PIPEWIRE_DO_TIMESTAMP, DEFAULT_PIPEWIRE_TIMEOUT, DaemonCore,
};
use crate::linux::pipewire::{PipewireManager, PipewireState};
use gstreamer::prelude::{ElementExt, ElementExtManual, GstBinExt, ObjectExt, PadExt};
use std::time::Duration;
use wayclip_core::models::error::WayclipError;
use wayclip_core::settings::UserSettings;
use wayclip_core::settings::recording::{AudioNode, AudioSettings};

// Not inside daemon core to avoid locks
pub fn init_default_audio_device(
    device_type: DefaultDeviceType,
    pipewire_state: &PipewireState,
) -> Result<Option<String>, WayclipError> {
    if device_type == DefaultDeviceType::Background
        && let Some(sink_node) = &pipewire_state.default_sink_node_name
    {
        UserSettings::load()?.set_str("recording.audio.background.node_name", sink_node)?;
        return Ok(Some(sink_node.clone()));
    }

    if device_type == DefaultDeviceType::Microphone
        && let Some(source_node) = &pipewire_state.default_source_node_name
    {
        UserSettings::load()?.set_str("recording.audio.microphone.node_name", source_node)?;
        return Ok(Some(source_node.clone()));
    }

    Ok(None)
}

impl DaemonCore {
    pub async fn check_audio_devices(
        pipewire_manager: &PipewireManager,
        audio_settings: &mut AudioSettings,
    ) -> Result<(), WayclipError> {
        let mut reciever = pipewire_manager.subscribe();

        let (microphone_node_name, background_node_name) = (
            audio_settings.microphone.node_name.clone(),
            audio_settings.background.node_name.clone(),
        );

        let state =
            match tokio::time::timeout(Duration::from_secs(DEFAULT_PIPEWIRE_TIMEOUT), async {
                loop {
                    let state = reciever.borrow().clone();

                    let mic_found = microphone_node_name.is_empty()
                        || state
                            .devices
                            .iter()
                            .any(|d| d.node_name == microphone_node_name);
                    let bg_found = background_node_name.is_empty()
                        || state
                            .devices
                            .iter()
                            .any(|d| d.node_name == background_node_name);

                    if mic_found && bg_found {
                        return state;
                    }

                    if reciever.changed().await.is_err() {
                        return state;
                    }
                }
            })
            .await
            {
                Ok(state) => state,
                Err(_) => pipewire_manager.current_state(),
            };

        log::debug!("Pipewire manager state: {:?}", state);

        if microphone_node_name.is_empty() {
            init_default_audio_device(DefaultDeviceType::Microphone, &state)?;
        }

        if background_node_name.is_empty() {
            init_default_audio_device(DefaultDeviceType::Background, &state)?;
        }

        if !pipewire_manager.is_node_name_valid(microphone_node_name.as_str())
            && audio_settings.microphone.enabled
        {
            return Err(WayclipError::Audio(
                format!("Audio device {} doesnt exist", microphone_node_name).into(),
            ));
        };

        if !pipewire_manager.is_node_name_valid(background_node_name.as_str())
            && audio_settings.background.enabled
        {
            return Err(WayclipError::Audio(
                format!("Audio device {} doesnt exist", background_node_name).into(),
            ));
        };

        Ok(())
    }

    pub fn build_audio_source_pipeline(
        &self,
        pipeline: &gstreamer::Pipeline,
        audio_node: &AudioNode,
        audio_node_type: DefaultDeviceType,
        sample_rate_hz: u64,
        mix: &gstreamer::Element,
    ) -> Result<(), WayclipError> {
        // first check if existant node
        let node_id = &self
            .pipewire_manager
            .get_node_id_from_node_name(audio_node.node_name.as_str());

        if audio_node.enabled
            && let Some(id) = node_id
        {
            // Sanitize node name to use in element names
            let safe_name = audio_node
                .node_name
                .replace(|c: char| !c.is_alphanumeric(), "_");

            let pipewiresrc = gstreamer::ElementFactory::make("pipewiresrc")
                .property("do-timestamp", DEFAULT_PIPEWIRE_DO_TIMESTAMP)
                .property("target-object", id.to_string().as_str())
                .name(format!("audio_src_{}", safe_name))
                .build()?;

            if audio_node_type == DefaultDeviceType::Background {
                let properties = gstreamer::Structure::builder("properties")
                    .field("stream.capture.sink", true)
                    .build();
                pipewiresrc.set_property("stream-properties", &properties);
            }

            let queue = gstreamer::ElementFactory::make("queue")
                .name(format!("audio_queue_{}", safe_name))
                .build()?;

            let caps_filter = gstreamer::ElementFactory::make("capsfilter")
                .name(format!("audio_caps_{}", safe_name))
                .build()?;

            let caps = gstreamer::Caps::builder("audio/x-raw")
                .field("rate", sample_rate_hz as i32)
                .field("channels", DEFAULT_AUDIO_CHANNELS)
                .build();

            caps_filter.set_property("caps", &caps);

            let audioconvert = gstreamer::ElementFactory::make("audioconvert")
                .name(format!("audio_convert_{}", safe_name))
                .build()?;
            let audioresample = gstreamer::ElementFactory::make("audioresample")
                .name(format!("audio_resample_{}", safe_name))
                .build()?;

            let mix_sink_pad = mix
                .request_pad_simple("sink_%u")
                .ok_or_else(|| WayclipError::Audio("Couldnt request mix sink pad".into()))?;

            mix_sink_pad.set_property("volume", audio_node.level.0);

            let audio_source_pipeline = [
                pipewiresrc,
                queue,
                caps_filter,
                audioconvert,
                audioresample.clone(),
            ];

            for element in audio_source_pipeline.iter() {
                pipeline.add(element)?
            }

            gstreamer::Element::link_many(
                audio_source_pipeline
                    .iter()
                    .collect::<Vec<&gstreamer::Element>>()
                    .as_slice(),
            )?;

            let resample_src_pad = audioresample
                .static_pad("src")
                .ok_or_else(|| WayclipError::Audio("Couldnt request src pad".into()))?;

            resample_src_pad.link(&mix_sink_pad)?;
        }

        Ok(())
    }
}
