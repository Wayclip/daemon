use crate::linux::ipc::{commands::IpcCommand, server::DaemonServer};
use tokio::sync::mpsc;
use wayclip_core::models::error::WayclipError;
use zbus::Connection;

pub mod client;
pub mod commands;
pub mod server;

pub const DEFAULT_SYSTEMD_SERVICE: &str = "wayclip-daemon.service";
pub const DEFAULT_DBUS_SERVICE: &str = "org.wayclip.Daemon";
pub const DEFAULT_MODE: &str = "replace";
pub const DEFAULT_INTERFACE_PATH: &str = "/org/wayclip/Daemon";
pub const DEFAULT_DBUS_INTERFACE: &str = "org.wayclip.Daemon1";

/// DaemonIpc will be responsible for communications with the daemon.
pub struct DaemonIpc {
    _connection: Option<Connection>,
    pub command_sender: mpsc::Sender<IpcCommand>,
    command_receiver: mpsc::Receiver<IpcCommand>,
}

impl DaemonIpc {
    // On new, we only create a channel
    pub fn new() -> Result<Self, WayclipError> {
        let (sender, receiver) = mpsc::channel(32);

        Ok(Self {
            _connection: None,
            command_sender: sender,
            command_receiver: receiver,
        })
    }

    pub async fn recieve(&mut self) -> Option<IpcCommand> {
        self.command_receiver.recv().await
    }

    // Then on connect, we actually attempt to make a connection & store that inside
    pub async fn connect(&mut self) -> Result<(), WayclipError> {
        let server = DaemonServer {
            command_sender: self.command_sender.clone(),
        };

        let connection = zbus::connection::Builder::session()?
            .name(DEFAULT_DBUS_SERVICE)?
            .serve_at(DEFAULT_INTERFACE_PATH, server)?
            .build()
            .await?;

        self._connection = Some(connection);
        Ok(())
    }
}
