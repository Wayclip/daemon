use ashpd::{
    desktop::{
        CreateSessionOptions, PersistMode, Session,
        screencast::{
            CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
            StartCastOptions,
        },
    },
    enumflags2::BitFlags,
};
use gstreamer::glib::object::ObjectExt;
use std::{
    fs::{create_dir_all, read_to_string, write},
    os::fd::{AsRawFd, OwnedFd},
};
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

use crate::{
    common::gst::{
        GStreamer, app::DEFAULT_APPSRC_DO_TIMESTAMP, caps::GStreamerCapsType,
        element::GStreamerElementType, pipeline::GStreamerPipeline,
    },
    linux::{
        core::engine::pipewire::manager::PipewireManager,
        core1::{DEFAULT_AUDIO_CHANNELS, types::DefaultDeviceType},
    },
};

const DEFAULT_SOURCE_TYPE: SourceType = SourceType::Monitor;
const DEFAULT_RESTORE_TOKEN_PATH: &str = "wayclip/restore_token";
const DEFAULT_CURSOR_MODE: CursorMode = CursorMode::Embedded;
const DEFAULT_PERSIST_MODE: PersistMode = PersistMode::ExplicitlyRevoked;

pub mod manager;

// We use ashpd to capute the screen, however, the input is provided by pipewire anyway
pub struct DaemonEnginePipewire {
    manager: PipewireManager,
    connection_data: DaemonEngineConnectionData,
}

#[derive(Default)]
pub struct DaemonEngineConnectionData {
    proxy: Option<Screencast>,
    session: Option<Session<Screencast>>,
    file_descriptor: Option<OwnedFd>,
    node_id: Option<String>,
    restore_token: Option<String>,
}

impl DaemonEngineConnectionData {
    /// Returns the ((raw) FileDescriptor, NodeID) and errors if not present
    pub fn extract_data(&self) -> Result<(i32, String), WayclipError> {
        Ok((
            self.file_descriptor
                .as_ref()
                .ok_or_else(|| WayclipError::NotFound("No file descriptor was found".into()))?
                .as_raw_fd(),
            self.node_id
                .as_ref()
                .ok_or_else(|| WayclipError::NotFound("No node id was found".into()))?
                .clone(),
        ))
    }
}

impl DaemonEnginePipewire {
    pub fn new() -> Result<Self, WayclipError> {
        Ok(Self {
            // We initialise the pipewire manager, so that we can have constant access to it
            // allowing us to pull info about devices and more
            manager: PipewireManager::new()?,
            // Rest of variables are None, since we are only creating the instance and have not yet
            // captured any information
            connection_data: DaemonEngineConnectionData::default(),
        })
    }

    pub async fn stop(&mut self) -> Result<(), WayclipError> {
        self.connection_data.node_id = None;
        self.connection_data.file_descriptor = None;
        self.connection_data.proxy = None;

        if let Some(session) = self.connection_data.session.take() {
            session.close().await?;
        }

        Ok(())
    }

    /// This method will initialise all the audio-related things...
    /// This method will query the current pipewire state (which by the time this method is called
    /// should have already collected enough data).
    /// We will try to find the specified node names in current pipewire state. If fail -> display
    /// error & use default devices.
    /// This method is not responsible for CHANING user settings if something is wrong. Only using
    /// defaults to avoid fatal errors.
    pub fn audio_setup(
        &mut self,
        pipeline: &GStreamerPipeline,
        user_settings: &UserSettings,
        sink_pad: &gstreamer::Pad,
    ) -> Result<(), WayclipError> {
        let state = self.manager.current_state();
        let audio = &user_settings.recording.audio;

        if audio.background.enabled {
            let (node_name, node_level) = match self
                .manager
                .is_node_name_valid(&audio.background.node_name, manager::PipewireNodeType::Sink)
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

            let node_id = self
                .manager
                .get_node_id_from_node_name(node_name)
                .ok_or_else(|| WayclipError::NotFound("No ID found for the sink".into()))?;

            self.setup_audio_device(
                pipeline,
                user_settings,
                node_id,
                node_level,
                DefaultDeviceType::Background,
                sink_pad,
            )?;
        }

        if audio.microphone.enabled {
            let (node_name, node_level) = match self.manager.is_node_name_valid(
                &audio.microphone.node_name,
                manager::PipewireNodeType::Source,
            ) {
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

            let node_id = self
                .manager
                .get_node_id_from_node_name(node_name)
                .ok_or_else(|| WayclipError::NotFound("No ID found for the source".into()))?;

            self.setup_audio_device(
                pipeline,
                user_settings,
                node_id,
                node_level,
                DefaultDeviceType::Microphone,
                sink_pad,
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
        &mut self,
        pipeline: &GStreamerPipeline,
        user_settings: &UserSettings,
        audio_node_id: u32,
        audio_node_level: f64,
        audio_node_type: DefaultDeviceType,
        sink_pad: &gstreamer::Pad,
    ) -> Result<(), WayclipError> {
        let pipewire_src = GStreamer::build_element(GStreamerElementType::AudioPipewireSrc {
            do_timestamp: DEFAULT_APPSRC_DO_TIMESTAMP,
            target_object: audio_node_id.to_string().into(),
            sink: audio_node_type.is_sink(),
        })?;

        let queue = GStreamer::build_element(GStreamerElementType::AudioQueue)?;

        let caps = GStreamer::build_caps(GStreamerCapsType::AudioXRaw {
            rate: user_settings.recording.audio.sample_rate_hz.0 as i32,
            channels: DEFAULT_AUDIO_CHANNELS,
        });
        let caps_filter = GStreamer::build_element(GStreamerElementType::CapsFilter { caps })?;

        let audioconvert = GStreamer::build_element(GStreamerElementType::AudioConvert)?;
        let audioresample = GStreamer::build_element(GStreamerElementType::AudioResample)?;

        sink_pad.set_property("volume", audio_node_level);

        pipeline.add_and_link(&[
            &pipewire_src,
            &queue,
            &audioconvert,
            &audioresample,
            &caps_filter,
        ])?;

        let src_pad = GStreamer::get_static_pad(&audioresample, "src")?;
        GStreamer::link_pads(&src_pad, &sink_pad)?;

        Ok(())
    }

    // Our main entry point to setup screencast
    pub async fn setup_screncast(&mut self) -> Result<(), WayclipError> {
        // Create proxy & session so we can communicate with xdg-portal and call methods
        let proxy = Screencast::new().await?;
        let session = proxy
            .create_session(CreateSessionOptions::default())
            .await?;

        // Try to get avaialble modes
        let mode = if proxy
            .available_cursor_modes()
            .await?
            .contains(DEFAULT_CURSOR_MODE)
        {
            DEFAULT_CURSOR_MODE
        } else {
            CursorMode::Hidden
        };

        // Attempt to load an existing token from ~/.local/state
        let existing_token = self.load_restore_token()?;

        let select_sources_options = SelectSourcesOptions::default()
            .set_cursor_mode(mode)
            .set_restore_token(existing_token.as_deref())
            .set_persist_mode(DEFAULT_PERSIST_MODE)
            .set_multiple(false)
            .set_sources(BitFlags::from(DEFAULT_SOURCE_TYPE));

        proxy
            .select_sources(&session, select_sources_options)
            .await?;

        // Request a select from user -- this is the interactive step
        let start_request = proxy
            .start(&session, None, StartCastOptions::default())
            .await?;

        // query streams & extract data
        let streams = start_request.response()?;
        let stream = streams.streams().first().ok_or_else(|| {
            WayclipError::Screencast("Could not extract first stream in response".into())
        })?;

        let node_id = stream.pipe_wire_node_id().to_string();
        let file_descriptor = proxy
            .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
            .await?;

        // now extract token from streams & save it
        let restore_token = streams.restore_token();
        if let Some(token) = restore_token {
            self.save_restore_token(&token)?;
        }

        self.connection_data = DaemonEngineConnectionData {
            proxy: Some(proxy),
            session: Some(session),
            file_descriptor: Some(file_descriptor),
            node_id: Some(node_id),
            restore_token: restore_token.map(|t| t.to_string()),
        };

        Ok(())
    }

    fn load_restore_token(&self) -> Result<Option<String>, WayclipError> {
        let state_dir = dirs::state_dir().ok_or_else(|| {
            WayclipError::NotFound("Couldnt get state directory (~/.local/state)".into())
        })?;

        let path = state_dir.join(DEFAULT_RESTORE_TOKEN_PATH);
        if path.exists() {
            let token = read_to_string(path)?.trim().to_string();
            if !token.is_empty() {
                return Ok(Some(token));
            }
        }
        Ok(None)
    }

    fn save_restore_token(&self, token: &str) -> Result<(), WayclipError> {
        let state_dir = dirs::state_dir().ok_or_else(|| {
            WayclipError::NotFound("Couldnt get state directory (~/.local/state)".into())
        })?;

        let path = state_dir.join(DEFAULT_RESTORE_TOKEN_PATH);
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        write(path, token)?;

        Ok(())
    }
}
