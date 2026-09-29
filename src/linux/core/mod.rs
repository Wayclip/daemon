use std::{io::IsTerminal, process::exit, time::Duration};

use sd_notify::NotifyState;
use tokio::{
    signal::unix::{self, SignalKind},
    time::interval,
};
use tokio_util::sync::CancellationToken;
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

use crate::{
    common::misc::notifications::{NotificationEvent, NotificationManager},
    linux::{
        controller::ControllerManager,
        core::{
            engine::DaemonEngine,
            ipc::{DaemonIpc, commands::IpcCommand},
            services::DaemonServices,
            session::CurrentSession,
        },
        core1::types::DaemonStatus,
        desktop::DesktopEnvironmentManager,
        tray::WayclipTray,
    },
};

pub mod engine;
pub mod ipc;
pub mod services;
pub mod session;

/// The DaemonCore will act as the central orchistrator for the whole of daemon
pub struct DaemonCore {
    /// The ID is used to compare generations, or instances of the Daemon
    id: String,
    /// Status will be updated as the process goes through its life cycle
    status: DaemonStatus,
    /// The engine is the main driver in the daemon, handing connections, recording & frame storing
    engine: DaemonEngine,
    /// Services will contain all the additional servies and methods that are run in between
    /// capturing frames, like sending discord status, or detecting game
    services: DaemonServices,
    /// IPC module responsible for communications
    ipc: DaemonIpc,
    /// Current Session stores dat about the current recording session, like the user, clip count &
    /// more
    current_session: CurrentSession,
}

impl DaemonCore {
    pub fn new() -> Result<Self, WayclipError> {
        // Since the daemon will be standalone, this will be the 'entrypoint', meaning we will have
        // to pull fresh settings, set locale & more
        let user_settings = UserSettings::load()?;
        wayclip_core::set_locale(&user_settings.output.language.to_string());

        Ok(Self {
            id: nanoid::nanoid!(),
            status: DaemonStatus::Inactive,
            engine: DaemonEngine::new(user_settings.recording.video.get_max_duration())?,
            services: DaemonServices::new(&user_settings)?,
            ipc: DaemonIpc::new()?,
            current_session: CurrentSession::new(user_settings, None),
        })
    }

    /// After creating our DaemonCore, we can start the whole system, putting it into a running
    /// state, where we will recording, accept IPC calls, and wait for further termination input.
    /// This will be a wrapper arouund setup() so we can change state if something fails inside
    pub async fn start(&mut self) -> Result<(), WayclipError> {
        // Put ourselves into the activating status & send notification to user that daemon is
        // starting
        self.update_status(DaemonStatus::Activating)?;
        // Notification Manager does not need to be held persistently
        NotificationManager::send_event(
            NotificationEvent::DaemonStart,
            &self.current_session.user_settings.notification,
            String::default(),
        )?;

        // We create a cancellation token & attempt to setup our daemon
        let cancel_token = CancellationToken::new();
        if let Err(e) = self.setup(cancel_token.clone()).await {
            log::error!("error during setup: {}. shutting down...", e);
            self.stop().await?;
            exit(1);
        }

        self.update_status(DaemonStatus::Active)?;

        // After succesful start, we start our infinite event loop
        self.event_loop(cancel_token).await?;

        Ok(())
    }

    /// Stop is our new graceful shutdown procedure, which handles properly stopping
    pub async fn stop(&mut self) -> Result<(), WayclipError> {
        // Mark as deactivating
        self.update_status(DaemonStatus::Deactivating);

        // stop the recording engine, close session and portal
        self.engine.stop().await?;

        // remove the auto-bind
        let mut desktop = DesktopEnvironmentManager::new(
            self.current_session
                .user_settings
                .shortcuts
                .save_clip
                .clone(),
        )?;
        desktop.remove_auto_bind()?;

        // For debug in ring buffer
        if !std::io::stderr().is_terminal() {
            eprint!("\r\x1b[2K");
        }

        // send notification after done & mark as fully inactive
        NotificationManager::send_event(
            NotificationEvent::DaemonStop,
            &self.current_session.user_settings.notification,
            String::default(),
        )?;
        self.update_status(DaemonStatus::Inactive)?;

        Ok(())
    }

    async fn setup(&mut self, cancel_token: CancellationToken) -> Result<(), WayclipError> {
        // Then, attempt to create a new connection so that processes can communicate with us
        self.ipc.connect().await?;

        let mut desktop = DesktopEnvironmentManager::new(
            self.current_session
                .user_settings
                .shortcuts
                .save_clip
                .clone(),
        )?;
        desktop.create_auto_bind(self.ipc.command_sender.clone())?;

        if let Some(bind) = self
            .current_session
            .user_settings
            .shortcuts
            .save_clip_controller
            .clone()
        {
            ControllerManager::start(bind, cancel_token.clone(), self.ipc.command_sender.clone())?;
        }

        WayclipTray::run_tray(
            self.ipc.command_sender.clone(),
            self.current_session.user_settings.tray.clone(),
            cancel_token,
        );

        // ...build pipelines
        // ...start watcher

        Ok(())
    }

    /// Even loop will watch for SIGINT, SIGTERM, in addition to handling any IPC messages and
    /// calling update() on services
    async fn event_loop(&mut self, cancel_token: CancellationToken) -> Result<(), WayclipError> {
        let mut tick = interval(Duration::from_millis(500));
        let mut sigint = unix::signal(SignalKind::interrupt())?;
        let mut sigterm = unix::signal(SignalKind::terminate())?;

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                _ = sigint.recv() => {
                    log::info!("SIGINT received, stopping...");
                    break;
                }
                _ = sigterm.recv() => {
                    log::info!("SIGTERM received, stopping...");
                    break;
                }

                _ = tick.tick() => {
                    // self.services.update(&mut self.current_session).await?;
                }

                Some(ipc_command) = self.ipc.recieve() => {
                    self.handle_ipc_command(ipc_command).await?;
                }
            }
        }

        Ok(())
    }

    /// Once an IPC message is recieved, we can handle it direcrtly here, so that we can call method
    /// ssuch as stop(), and more
    async fn handle_ipc_command(&mut self, ipc_command: IpcCommand) -> Result<(), WayclipError> {
        match ipc_command {
            IpcCommand::Shutdown { .. } => self.stop().await?,
            _ => (),
        }

        Ok(())
    }

    /// We need a separate method to handle changing statuses, because some will require just
    /// changing daemon status field, whilst others will also require sending an sd_notify command
    /// to systemd
    fn update_status(&mut self, status: DaemonStatus) -> Result<(), WayclipError> {
        match status {
            DaemonStatus::Inactive => {
                self.status = DaemonStatus::Inactive;
                sd_notify::notify(&[NotifyState::Stopping])?;
            }
            DaemonStatus::Active => {
                self.status = DaemonStatus::Active;
                sd_notify::notify(&[NotifyState::Ready])?;
            }
            s => self.status = s,
        }

        Ok(())
    }
}
