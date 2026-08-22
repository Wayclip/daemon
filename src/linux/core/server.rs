use crate::linux::core::DaemonCore;
use crate::linux::core::types::DaemonStatus;
use std::sync::Arc;
use zbus::fdo;
use zbus::interface;

pub struct DaemonServer {
    pub inner: Arc<tokio::sync::Mutex<DaemonCore>>,
}

#[interface(name = "org.wayclip.Daemon1")]
impl DaemonServer {
    #[zbus(name = "GetStatus")]
    async fn get_status(&self) -> fdo::Result<DaemonStatus> {
        let daemon = self.inner.lock().await;
        daemon
            .get_status()
            .await
            .map_err(|e| fdo::Error::Failed(format!("get_status failed: {e}")))
    }

    #[zbus(name = "SaveClip")]
    async fn save_clip(&self) -> fdo::Result<()> {
        DaemonCore::save_clip(self.inner.clone(), None)
            .await
            .map_err(|e| fdo::Error::Failed(format!("save_clip failed: {e}")))
    }

    #[zbus(name = "SaveClipWithCustomName")]
    async fn save_clip_with_custom_name(&self, forced_name: String) -> fdo::Result<()> {
        DaemonCore::save_clip(self.inner.clone(), Some(forced_name))
            .await
            .map_err(|e| fdo::Error::Failed(format!("save_clip failed: {e}")))
    }

    #[zbus(name = "RescanGames")]
    async fn rescan_games(&self) -> fdo::Result<(String, f32)> {
        let mut daemon = self.inner.lock().await;
        Ok(daemon.rescan_games().await)
    }
}
