use crate::linux::core::ipc::commands::IpcCommand;
use crate::linux::core1::types::DaemonStatus;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use zbus::fdo;
use zbus::interface;

/// DaemonServer will just be responsible for handdling oneshot channels on every IpcCommand. Inside
/// each command, we have a responder, which is the sender for the channle we just created. This
/// way, the other side can communicate directly with this module
pub struct DaemonServer {
    pub command_sender: mpsc::Sender<IpcCommand>,
}

//impl DaemonServer {
//    async fn send(&self, command: IpcCommand) -> Result<(), WayclipError> {
//        let (sender, receiver) = oneshot::channel();
//        self.command_sender.send(command)
//
//    }
//}

#[interface(name = "org.wayclip.Daemon1")]
impl DaemonServer {
    #[zbus(name = "GetStatus")]
    async fn get_status(&self) -> fdo::Result<DaemonStatus> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender
            .send(IpcCommand::GetStatus { responder: sender })
            .await
            .map_err(|_| fdo::Error::Failed("Daemon core inactive".into()))?;

        receiver
            .await
            .map_err(|_| fdo::Error::Failed("Core failed to respond".into()))
    }

    #[zbus(name = "SaveClip")]
    async fn save_clip(&self) -> fdo::Result<()> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender
            .send(IpcCommand::SaveClip {
                custom_name: None,
                responder: sender,
            })
            .await
            .map_err(|_| fdo::Error::Failed("Daemon core inactive".into()))?;

        receiver
            .await
            .map_err(|_| fdo::Error::Failed("Core failed to respond".into()))?
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    #[zbus(name = "SaveClipWithCustomName")]
    async fn save_clip_with_custom_name(&self, forced_name: String) -> fdo::Result<()> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender
            .send(IpcCommand::SaveClip {
                custom_name: Some(forced_name),
                responder: sender,
            })
            .await
            .map_err(|_| fdo::Error::Failed("Daemon core inactive".into()))?;

        receiver
            .await
            .map_err(|_| fdo::Error::Failed("Core failed to respond".into()))?
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    #[zbus(name = "RescanGames")]
    async fn rescan_games(&self) -> fdo::Result<(String, f32)> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender
            .send(IpcCommand::RescanGames { responder: sender })
            .await
            .map_err(|_| fdo::Error::Failed("Daemon core inactive".into()))?;

        receiver
            .await
            .map_err(|_| fdo::Error::Failed("Core failed to respond".into()))?
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    #[zbus(name = "Shutdown")]
    async fn shutdown(&self) -> fdo::Result<()> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender
            .send(IpcCommand::Shutdown { responder: sender })
            .await
            .map_err(|_| fdo::Error::Failed("Daemon core inactive".into()))?;

        receiver
            .await
            .map_err(|_| fdo::Error::Failed("Core failed to respond".into()))?
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }
}
