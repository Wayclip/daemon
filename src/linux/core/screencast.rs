use crate::linux::core::DaemonCore;
use ashpd::desktop::Session;
use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
    StartCastOptions, Streams,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode};
use ashpd::enumflags2::BitFlags;
use std::fs;
use std::os::fd::OwnedFd;
use wayclip_core::models::error::WayclipError;

const DEFAULT_SOURCE_TYPE: SourceType = SourceType::Monitor;
const DEFAULT_RESTORE_TOKEN_PATH: &str = "wayclip/restore_token";
const DEFAULT_PERSIST_MODE: PersistMode = PersistMode::ExplicitlyRevoked;

pub struct ScreencastNegotiation {
    pub pipewire_session: Session<Screencast>,
    pub pipewire_file_descriptor: OwnedFd,
    pub pipewire_node_id: String,
    pub pipewire_proxy: Screencast,
    pub restore_token: Option<String>,
}

impl DaemonCore {
    pub async fn negotiate_screencast() -> Result<ScreencastNegotiation, WayclipError> {
        // Create a new proxy + session
        let pipewire_proxy = Screencast::new().await?;
        let create_session_options = CreateSessionOptions::default();

        let pipewire_session = pipewire_proxy
            .create_session(create_session_options)
            .await?;

        // Try to get a token, if it exists
        let existing_restore_token = Self::load_restore_token()?;

        // Finally its cleaner
        let select_sources_options = SelectSourcesOptions::default()
            .set_cursor_mode(CursorMode::Embedded)
            .set_restore_token(existing_restore_token.as_deref())
            .set_persist_mode(DEFAULT_PERSIST_MODE)
            .set_multiple(false)
            .set_sources(BitFlags::from(DEFAULT_SOURCE_TYPE));

        // Request a select from user -- this is the interactive step. It waits
        // on the user picking (or ignoring) the portal's share dialog and has
        // no meaningful upper bound.
        pipewire_proxy
            .select_sources(&pipewire_session, select_sources_options)
            .await?;

        let start_cast_options = StartCastOptions::default();

        // Start the stream
        let stream_request = pipewire_proxy
            .start(&pipewire_session, None, start_cast_options)
            .await?;
        let all_streams = stream_request.response()?;
        let stream = all_streams.streams().first().ok_or_else(|| {
            WayclipError::Screencast("Could not extract first stream in response".into())
        })?;

        let pipewire_options = OpenPipeWireRemoteOptions::default();
        let pipewire_node_id = stream.pipe_wire_node_id().to_string();
        let pipewire_file_descriptor = pipewire_proxy
            .open_pipe_wire_remote(&pipewire_session, pipewire_options)
            .await?;

        let restore_token = Self::save_restore_token(&all_streams)?;

        Ok(ScreencastNegotiation {
            pipewire_session,
            pipewire_file_descriptor,
            pipewire_node_id,
            pipewire_proxy,
            restore_token,
        })
    }

    fn load_restore_token() -> Result<Option<String>, WayclipError> {
        let state_dir = dirs::state_dir().ok_or_else(|| {
            WayclipError::NotFound("Couldnt get state directory (~/.local/state)".into())
        })?;
        let path = state_dir.join(DEFAULT_RESTORE_TOKEN_PATH);
        if path.exists() {
            let token = fs::read_to_string(path)?.trim().to_string();
            if !token.is_empty() {
                return Ok(Some(token));
            }
        }
        Ok(None)
    }

    fn save_restore_token(all_streams: &Streams) -> Result<Option<String>, WayclipError> {
        if let Some(restore_token) = all_streams.restore_token() {
            let state_dir = dirs::state_dir().ok_or_else(|| {
                WayclipError::NotFound("Couldnt get state directory (~/.local/state)".into())
            })?;
            let path = state_dir.join(DEFAULT_RESTORE_TOKEN_PATH);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, restore_token)?;
            return Ok(Some(restore_token.to_string()));
        }
        Ok(None)
    }
}
