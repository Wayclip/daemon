use sd_notify::NotifyState;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    io::IsTerminal,
    process::exit,
    time::{Duration, Instant},
};
use tokio::{
    signal::unix::{self, SignalKind},
    sync::mpsc,
    time::interval,
};
use tokio_util::sync::CancellationToken;
use wayclip_core::{models::error::WayclipError, settings::UserSettings};
use zbus::zvariant::Type;

use crate::{
    common::{
        gst::bus::CoreEvent,
        misc::notifications::{NotificationEvent, NotificationManager},
    },
    linux::{
        engine::{
            DaemonEngine, STALL_LIMIT,
            gstreamer::save::{SaveDone, SavePipelineFactory},
        },
        ipc::{DaemonIpc, commands::IpcCommand},
        services::DaemonServices,
        session::CurrentSession,
    },
};

const MAX_RECOVERIES: usize = 3;
const RECOVERY_WINDOW: Duration = Duration::from_secs(300);
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(30);

pub mod doctor;
pub mod engine;
pub mod ipc;
pub mod services;
pub mod session;

#[derive(Debug, Clone, PartialEq, Eq, Type, Serialize, Deserialize)]
pub enum DaemonStatus {
    Active,
    Inactive,
    Saving,
    Activating,
    Deactivating,
    Failed,
}

/// The DaemonCore will act as the central orchistrator for the whole of daemon
pub struct DaemonCore {
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
    /// We have a token to stop the process
    cancel_token: CancellationToken,
    /// We setup a new channel (in addition to the IPC channel -- although consider merging?) to
    /// monitor events and actions needed to take (errors/recovery)
    event_channel: (mpsc::Sender<CoreEvent>, mpsc::Receiver<CoreEvent>),
    /// We also setup another channel to communicate with Saves so that they can run in BG and they
    /// dont block main pipeline
    save_channel: (mpsc::Sender<SaveDone>, mpsc::Receiver<SaveDone>),
    /// We track all the recoveries we attempted
    recoveries: VecDeque<Instant>,
}

impl DaemonCore {
    pub fn new() -> Result<Self, WayclipError> {
        // Since the daemon will be standalone, this will be the 'entrypoint', meaning we will have
        // to pull fresh settings, set locale & more
        let user_settings = UserSettings::load()?;
        wayclip_core::set_locale(&user_settings.output.language.to_string());

        let cancel_token = CancellationToken::new();

        let event_channel = mpsc::channel(1);
        let save_channel = mpsc::channel(4);

        Ok(Self {
            status: DaemonStatus::Inactive,
            engine: DaemonEngine::new(user_settings.recording.video.get_max_duration())?,
            services: DaemonServices::new(&user_settings, cancel_token.clone())?,
            ipc: DaemonIpc::new()?,
            current_session: CurrentSession::new(user_settings, None),
            cancel_token,
            event_channel,
            save_channel,
            recoveries: VecDeque::new(),
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
        if let Err(e) = self.setup().await {
            log::error!("error during setup: {}. shutting down...", e);
            self.stop().await?;
            exit(1);
        }

        self.update_status(DaemonStatus::Active)?;

        // After succesful start, we start our infinite event loop
        self.event_loop(self.cancel_token.clone()).await
    }

    async fn shutdown(&mut self, exit_code: i32) -> ! {
        if let Err(e) = self.stop().await {
            log::error!("error during shutdown: {e}");
        }
        exit(exit_code)
    }

    /// Stop is our new graceful shutdown procedure, which handles properly stopping
    pub async fn stop(&mut self) -> Result<(), WayclipError> {
        // Mark as deactivating
        self.update_status(DaemonStatus::Deactivating)?;

        // stop the recording engine, close session and portal
        let engine_res = self.engine.stop().await;

        // remove the auto-bind
        let services_res = self.services.stop_services().await;

        // For debug in ring buffer
        if std::io::stderr().is_terminal() {
            eprint!("\r\x1b[2K");
        }

        // send notification after done & mark as fully inactive
        NotificationManager::send_event(
            NotificationEvent::DaemonStop,
            &self.current_session.user_settings.notification,
            String::default(),
        )?;
        self.update_status(DaemonStatus::Inactive)?;

        engine_res.and(services_res)
    }

    async fn setup(&mut self) -> Result<(), WayclipError> {
        // Then, attempt to create a new connection so that processes can communicate with us
        self.ipc.connect().await?;

        self.services.start_services(
            &self.current_session.user_settings,
            &self.ipc.command_sender,
        )?;

        self.engine
            .setup(
                &self.current_session.user_settings,
                self.event_channel.0.clone(),
            )
            .await?;

        Ok(())
    }

    /// Even loop will watch for SIGINT, SIGTERM, in addition to handling any IPC messages and
    /// calling update() on services
    async fn event_loop(&mut self, cancel_token: CancellationToken) -> Result<(), WayclipError> {
        let mut tick = interval(Duration::from_millis(500));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut sigint = unix::signal(SignalKind::interrupt())?;
        let mut sigterm = unix::signal(SignalKind::terminate())?;

        loop {
            tokio::select! {
                // These 3 will shutdown daemon
                _ = cancel_token.cancelled() => {
                    log::info!("CancellationToken received, stopping...");
                    self.shutdown(0).await;
                },
                _ = sigint.recv() => {
                    log::info!("SIGINT received, stopping...");
                    self.shutdown(0).await;
                }
                _ = sigterm.recv() => {
                    log::info!("SIGTERM received, stopping...");
                    self.shutdown(0).await;
                }

                Some(event) = self.event_channel.1.recv() => {
                    log::error!("pipeline event: {:?}", event);
                    let reason = match event {
                        CoreEvent::PipelineError(m) => m,
                        CoreEvent::PipelineEos => "unexpected EOS".to_string(),
                    };
                    self.try_recover(&reason).await;
                }

                Some(SaveDone { result, responder }) = self.save_channel.1.recv() => {
                    self.status = DaemonStatus::Active;
                    match &result {
                        Ok(name) => {
                            // TODO: Automatic Uploads
                            self.current_session.new_clip(name.to_owned(), None);
                            let _ = self.services.saving(self.current_session.clone());
                            let _ = NotificationManager::send_event(
                                NotificationEvent::SaveSuccess,
                                &self.current_session.user_settings.notification,
                                name.to_owned(),
                            );
                        }
                        Err(e) => {
                            let _ = NotificationManager::send_event(
                                NotificationEvent::SaveError,
                                &self.current_session.user_settings.notification,
                                e.to_string(),
                            );
                        }
                    }
                    let _ = responder.send(result);
                }

                _ = tick.tick() => {
                    if let Err(e) = self.services.update(&mut self.current_session) {
                        log::error!("service update failed: {e}");
                    }
                    if self.status == DaemonStatus::Active && self.engine.is_stalled(STALL_LIMIT) {
                        log::error!("no video frames for {}s", STALL_LIMIT.as_secs());
                        self.try_recover("video stalled").await;
                    }
                }

                Some(ipc_command) = self.ipc.recieve() => {
                    if let Err(e) = self.handle_ipc_command(ipc_command).await {
                        log::error!("ipc handler failed: {e}");
                    }
                }
            }
        }
    }

    /// Once an IPC message is recieved, we can handle it direcrtly here, so that we can call method
    /// ssuch as stop(), and more
    async fn handle_ipc_command(&mut self, ipc_command: IpcCommand) -> Result<(), WayclipError> {
        match ipc_command {
            IpcCommand::GetStatus { responder } => responder
                .send(self.status.clone())
                .map_err(|_| WayclipError::Validation("Could not send GetStatus OK".into()))?,
            IpcCommand::Shutdown { responder } => {
                let res = self.stop().await;
                if let Err(e) = &res {
                    log::error!("shutdown error: {e}");
                }
                let _ = responder.send(res);
                exit(0);
            }
            IpcCommand::SaveClip {
                custom_name,
                responder,
            } => {
                if self.status == DaemonStatus::Saving {
                    let _ = responder.send(Err(WayclipError::Validation("Already saving".into())));
                    return Ok(());
                }

                self.update_status(DaemonStatus::Saving)?;
                let session = self.current_session.clone();
                let ring = self.engine.ring();
                let sender = self.save_channel.0.clone();
                tokio::spawn(async move {
                    let result = SavePipelineFactory::save(&session, custom_name, ring).await;
                    let _ = sender.send(SaveDone { result, responder });
                });
                self.update_status(DaemonStatus::Active)?;
            }
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

    async fn try_recover(&mut self, reason: &str) {
        if let Err(e) = self.recover(reason).await {
            log::error!("unrecoverable ({e}) exiting...");
            let _ = self.stop().await;
            exit(1);
        }
    }

    async fn recover(&mut self, reason: &str) -> Result<(), WayclipError> {
        let now = Instant::now();
        self.recoveries
            .retain(|t| now.duration_since(*t) < RECOVERY_WINDOW);
        if self.recoveries.len() >= MAX_RECOVERIES {
            return Err(WayclipError::Validation(
                format!(
                    "{MAX_RECOVERIES} recoveries in {}s. last: {reason}",
                    RECOVERY_WINDOW.as_secs()
                )
                .into(),
            ));
        }

        self.recoveries.push_back(now);

        log::warn!("recovering: {reason}");
        //self.update_status(DaemonStatus::Activating);
        self.engine.stop_watcher();

        let settings = self.current_session.user_settings.clone();
        let sender = self.event_channel.0.clone();
        tokio::time::timeout(RECOVERY_TIMEOUT, self.engine.recover(&settings, sender))
            .await
            .map_err(|_| WayclipError::Validation("recovery timed out".into()))??;

        while self.event_channel.1.try_recv().is_ok() {}
        Ok(())
    }
}
