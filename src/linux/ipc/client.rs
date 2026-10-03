use crate::linux::{
    DaemonStatus,
    ipc::{DEFAULT_MODE, DEFAULT_SYSTEMD_SERVICE},
};
use wayclip_core::models::error::WayclipError;
use zbus::{Connection, proxy};

// Handles all systemd connections and calls
// This file will solely work on linux due to systemd

pub struct DaemonClient {
    connection: zbus::Connection,
}

impl DaemonClient {
    pub async fn new() -> Result<Self, WayclipError> {
        let connection = Connection::session().await?;
        Ok(Self { connection })
    }

    pub async fn start_daemon(&self) -> Result<(), WayclipError> {
        let systemd = SystemdManagerProxy::new(&self.connection).await?;
        systemd
            .start_unit(DEFAULT_SYSTEMD_SERVICE, DEFAULT_MODE)
            .await?;
        Ok(())
    }

    pub async fn stop_daemon(&self) -> Result<(), WayclipError> {
        let systemd = SystemdManagerProxy::new(&self.connection).await?;
        systemd
            .stop_unit(DEFAULT_SYSTEMD_SERVICE, DEFAULT_MODE)
            .await?;
        Ok(())
    }

    pub async fn restart_daemon(&self) -> Result<(), WayclipError> {
        let systemd = SystemdManagerProxy::new(&self.connection).await?;
        systemd
            .restart_unit(DEFAULT_SYSTEMD_SERVICE, DEFAULT_MODE)
            .await?;
        Ok(())
    }

    pub async fn get_proxy(&self) -> Result<DaemonProxy<'_>, WayclipError> {
        let proxy = DaemonProxy::new(&self.connection).await?;
        Ok(proxy)
    }

    pub async fn enable_autostart(&self) -> Result<(), WayclipError> {
        let systemd = SystemdManagerProxy::new(&self.connection).await?;
        systemd
            .enable_unit_files(vec![DEFAULT_SYSTEMD_SERVICE], false, true)
            .await?;
        Ok(())
    }

    pub async fn disable_autostart(&self) -> Result<(), WayclipError> {
        let systemd = SystemdManagerProxy::new(&self.connection).await?;
        systemd
            .disable_unit_files(vec![DEFAULT_SYSTEMD_SERVICE], false)
            .await?;
        Ok(())
    }
}

pub enum ShutdownReason {
    TrayExit,
    FatalError(String),
}

// DaemonProxy will handle communication between CLI/GUI and the DaemonInstance itself.
// And yes this is hardcoded, aint no one chaning this
#[proxy(
    interface = "org.wayclip.Daemon1",
    default_service = "org.wayclip.Daemon",
    default_path = "/org/wayclip/Daemon"
)]
pub trait Daemon {
    async fn get_status(&self) -> zbus::fdo::Result<DaemonStatus>;
    async fn save_clip(&self) -> zbus::fdo::Result<String>;
    async fn save_clip_with_custom_name(&self, forced_name: String) -> zbus::fdo::Result<String>;
    async fn shutdown(&self) -> zbus::fdo::Result<()>;
}

#[proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
pub trait SystemdManager {
    async fn start_unit(
        &self,
        name: &str,
        mode: &str,
    ) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;

    async fn stop_unit(
        &self,
        name: &str,
        mode: &str,
    ) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;

    async fn restart_unit(
        &self,
        name: &str,
        mode: &str,
    ) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;

    async fn enable_unit_files(
        &self,
        files: Vec<&str>,
        runtime: bool,
        force: bool,
    ) -> zbus::Result<(bool, Vec<(String, String, String)>)>;

    async fn disable_unit_files(
        &self,
        files: Vec<&str>,
        runtime: bool,
    ) -> zbus::Result<Vec<(String, String, String)>>;
}
