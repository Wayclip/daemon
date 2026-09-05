use crate::linux::controller::ControllerManager;
use crate::linux::core::DaemonCore;
use crate::linux::core::server::DaemonServer;
use crate::linux::core::types::DaemonStatus;
use crate::linux::core::types::RecordingConfig;
use crate::linux::desktop::DesktopEnvironmentManager;
use crate::linux::tray::WayclipTray;
use log::info;
use std::process::exit;
use std::sync::Arc;
use tokio::signal::unix::SignalKind;
use tokio::signal::unix::signal;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wayclip_core::models::error::WayclipError;
use wayclip_core::settings::UserSettings;
use zbus::{Connection, proxy};

// Handles all systemd connections and calls
// This file will solely work on linux due to systemd

pub const DEFAULT_SYSTEMD_SERVICE: &str = "wayclip-daemon.service";
pub const DEFAULT_DBUS_SERVICE: &str = "org.wayclip.Daemon";
pub const DEFAULT_MODE: &str = "replace";
pub const DEFAULT_INTERFACE_PATH: &str = "/org/wayclip/Daemon";

pub enum ShutdownReason {
    TrayExit,
    FatalError(String),
}

// DaemonManager will use systemd to start/stop/create/kill instances of the Daemon
pub struct DaemonManager {
    connection: zbus::Connection,
}

impl DaemonManager {
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

    pub async fn initialise_daemon(settings: UserSettings) -> Result<(), WayclipError> {
        log::debug!("Settings: {:?}", settings);

        let shortcut_string = settings.shortcuts.save_clip.to_string();
        log::debug!("Parsed Shortcut: {shortcut_string}");

        let recording_config = RecordingConfig {
            bitrate_kbps: settings.recording.video.bitrate_kbps,
            resolution: settings.recording.video.resolution,
            fps: settings.recording.video.fps,
            audio: settings.recording.audio.clone(),
            codec: settings.recording.video.codec.clone(),
        };

        let inner = Arc::new(tokio::sync::Mutex::new(DaemonCore::new(
            settings.recording.video.length_seconds,
            settings.notification.clone(),
            recording_config.clone(),
            settings.output.clone(),
            settings.game_discovery.discord_rich_presence,
        )?));

        // Yes we drop & rely on the pending await.
        let _connection = zbus::connection::Builder::session()?
            .name(DEFAULT_DBUS_SERVICE)?
            .serve_at(
                DEFAULT_INTERFACE_PATH,
                DaemonServer {
                    inner: inner.clone(),
                },
            )?
            .build()
            .await?;

        let (shutdown_sender, mut shutdown_reciever) = mpsc::channel::<ShutdownReason>(10);
        let cancel_token = CancellationToken::new();

        DaemonCore::init(
            inner.clone(),
            recording_config.clone(),
            settings.game_discovery.clone(),
            cancel_token.clone(),
            shutdown_sender.clone(),
        )
        .await?;

        let shortcut_daemon_reference = inner.clone();
        let mut desktop = DesktopEnvironmentManager::new(
            shortcut_daemon_reference,
            settings.shortcuts.save_clip,
        )?;

        desktop.create_auto_bind()?;

        if let Some(bind) = settings.shortcuts.save_clip_controller {
            let controller_shortcut_daemon_referance = inner.clone();
            ControllerManager::new(controller_shortcut_daemon_referance, bind)
                .setup(cancel_token.clone())?;
        }

        let tray_daemon_reference = inner.clone();
        let tray_config = settings.tray.clone();
        let cancel_token_clone = cancel_token.clone();
        let shutdown_sender_clone = shutdown_sender.clone();
        tokio::spawn(async move {
            info!("Starting System Tray...");
            if let Err(e) = WayclipTray::run_tray(
                tray_daemon_reference,
                tray_config,
                cancel_token_clone,
                shutdown_sender_clone,
            )
            .await
            {
                log::error!("Tray Error: {:?}", e);
            }
        });

        info!("Daemon up and running");

        // basically now we wait for Ctrl+C
        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sigterm = signal(SignalKind::terminate())?;
        //let mut sighup = signal(SignalKind::hangup())?;

        let mut exit_code = 0;

        tokio::select! {
            _ = sigint.recv() => info!("SIGINT (Ctrl+C) received, shutting down..."),
            _ = sigterm.recv() => info!("SIGTERM received, shutting down..."),
            reason = shutdown_reciever.recv() => {
                match reason {
                    Some(ShutdownReason::TrayExit) => {
                        info!("Tray exit");
                        exit_code = 0;
                    }
                    Some(ShutdownReason::FatalError(err)) => {
                        log::error!("Fatal error: {}", err);
                        exit_code = 1;
                    }
                    None => {
                        info!("Channel dropped");
                        exit_code = 1;
                    }
                }
            }
            //_ = sighup.recv() => info!("SIGHUP received, shutting down..."),
        }

        cancel_token.cancel();

        if let Err(e) = desktop.remove_auto_bind() {
            log::warn!("Failed to unbind hotkeys: {e:?}");
        }

        // once we recieved from one of those, we just call the graceful shutdown method
        let mut daemon = inner.lock().await;
        if let Err(e) = daemon.shutdown().await {
            log::error!("Error during graceful shutdown: {e:?}");
        }

        exit(exit_code);
    }
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
    async fn save_clip(&self) -> zbus::fdo::Result<()>;
    async fn save_clip_with_custom_name(&self, forced_name: String) -> zbus::fdo::Result<()>;
    async fn rescan_games(&self) -> zbus::fdo::Result<(String, f32)>;
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
