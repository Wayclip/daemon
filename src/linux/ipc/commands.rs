use crate::linux::DaemonStatus;
use tokio::sync::oneshot;
use wayclip_core::models::error::WayclipError;

/// Instead of holding a direct reference of the Daemon, we will just manage IpcCommand
pub enum IpcCommand {
    GetStatus {
        responder: oneshot::Sender<DaemonStatus>,
    },
    SaveClip {
        custom_name: Option<String>,
        responder: oneshot::Sender<Result<String, WayclipError>>,
    },
    Shutdown {
        responder: oneshot::Sender<Result<(), WayclipError>>,
    },
}
