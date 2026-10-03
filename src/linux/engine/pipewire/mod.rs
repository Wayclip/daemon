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
use std::{
    fs::{create_dir_all, read_to_string, write},
    os::fd::{AsRawFd, OwnedFd},
};
use wayclip_core::models::error::WayclipError;

use crate::linux::engine::pipewire::manager::PipewireManager;

const DEFAULT_SOURCE_TYPE: SourceType = SourceType::Monitor;
const DEFAULT_RESTORE_TOKEN_PATH: &str = "wayclip/restore_token";
const DEFAULT_CURSOR_MODE: CursorMode = CursorMode::Embedded;
const DEFAULT_PERSIST_MODE: PersistMode = PersistMode::ExplicitlyRevoked;

pub mod manager;

// We use ashpd to capute the screen, however, the input is provided by pipewire anyway
pub struct DaemonEnginePipewire {
    pub manager: PipewireManager,
    pub connection_data: DaemonEngineConnectionData,
}

#[derive(Default)]
pub struct DaemonEngineConnectionData {
    proxy: Option<Screencast>,
    session: Option<Session<Screencast>>,
    file_descriptor: Option<OwnedFd>,
    node_id: Option<String>,
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
            if let Err(e) = session.close().await {
                log::warn!("portal session close: {e}");
            }
        }

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
        if let Some(token) = streams.restore_token() {
            self.save_restore_token(&token)?;
        }

        self.connection_data = DaemonEngineConnectionData {
            proxy: Some(proxy),
            session: Some(session),
            file_descriptor: Some(file_descriptor),
            node_id: Some(node_id),
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
