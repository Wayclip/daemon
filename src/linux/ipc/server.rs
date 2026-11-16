use crate::linux::DaemonStatus;
use crate::linux::ipc::commands::IpcCommand;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use zbus::fdo;
use zbus::interface;

const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const SAVE_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

// a function (sadly not a method) to allow for timeouts to exist
async fn await_reply<T>(
    receiver: oneshot::Receiver<T>,
    timeout: Duration,
    operation: &str,
) -> fdo::Result<T> {
    match tokio::time::timeout(timeout, receiver).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err(fdo::Error::Failed("core failed to respond".into())),
        Err(_) => Err(fdo::Error::Failed(format!(
            "{operation} timed out after {}s",
            timeout.as_secs()
        ))),
    }
}

/// DaemonServer will just be responsible for handdling oneshot channels on every IpcCommand. Inside
/// each command, we have a responder, which is the sender for the channle we just created. This
/// way, the other side can communicate directly with this module
pub struct DaemonServer {
    pub command_sender: mpsc::Sender<IpcCommand>,
}

#[interface(name = "org.wayclip.Daemon1")]
impl DaemonServer {
    #[zbus(name = "GetStatus")]
    async fn get_status(&self) -> fdo::Result<DaemonStatus> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender
            .send(IpcCommand::GetStatus { responder: sender })
            .await
            .map_err(|_| fdo::Error::Failed("Daemon core inactive".into()))?;

        await_reply(receiver, STATUS_TIMEOUT, "GetStatus").await
    }

    #[zbus(name = "SaveClip")]
    async fn save_clip(&self) -> fdo::Result<String> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender
            .send(IpcCommand::SaveClip {
                custom_name: None,
                responder: sender,
            })
            .await
            .map_err(|_| fdo::Error::Failed("Daemon core inactive".into()))?;

        await_reply(receiver, SAVE_TIMEOUT, "SaveClip")
            .await?
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    #[zbus(name = "SaveClipWithCustomName")]
    async fn save_clip_with_custom_name(&self, forced_name: String) -> fdo::Result<String> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender
            .send(IpcCommand::SaveClip {
                custom_name: Some(forced_name),
                responder: sender,
            })
            .await
            .map_err(|_| fdo::Error::Failed("Daemon core inactive".into()))?;

        await_reply(receiver, SAVE_TIMEOUT, "SaveClipWithCustomName")
            .await?
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    #[zbus(name = "Shutdown")]
    async fn shutdown(&self) -> fdo::Result<()> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender
            .send(IpcCommand::Shutdown { responder: sender })
            .await
            .map_err(|_| fdo::Error::Failed("Daemon core inactive".into()))?;

        await_reply(receiver, SHUTDOWN_TIMEOUT, "Shutdown")
            .await?
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }
}
