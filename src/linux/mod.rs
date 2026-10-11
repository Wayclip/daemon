use sd_notify::NotifyState;
use std::{
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
use wayclip_core::{
    models::{
        daemon::{
            DaemonStatus,
            notifications::{NotificationEvent, NotificationManager},
        },
        error::WayclipError,
    },
    settings::UserSettings,
};

use crate::{
    common::gst::bus::CoreEvent,
    linux::{
        engine::{
            DaemonEngine, STALL_LIMIT,
            gstreamer::{
                CaptureMode,
                save::{SaveDone, SavePipelineFactory},
            },
        },
        ipc::{DaemonIpc, commands::IpcCommand},
        memory::MemoryManager,
        services::DaemonServices,
        session::CurrentSession,
    },
};

const RECOVERY_TIMEOUT: Duration = Duration::from_secs(30);
const RECOVERY_BACKOFF: [Duration; 6] = [
    Duration::from_secs(0),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(30),
];
const HEALTHY_RESET: Duration = Duration::from_secs(30);
const REASONS: [&str; 4] = [
    "out_of_memory",
    "out of memory",
    "failed to upload buffer",
    "not_supported_device",
];

pub mod engine;
pub mod ipc;
pub mod memory;
pub mod services;
pub mod session;

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
    /// How many recoveries in a row were needed. Picks the backoff delay and how drastic the next
    /// recovery is. Reset once video has been flowing for HEALTHY_RESET.
    recovery_attempt: usize,
    /// A recovery that has been asked for and runs once its delay has passed
    recovery_due: Option<(Instant, String)>,
    healthy_since: Option<Instant>,
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
            recovery_attempt: 0,
            recovery_due: None,
            healthy_since: None,
        })
    }

    /// After creating our DaemonCore, we can start the whole system, putting it into a running
    /// state, where we will recording, accept IPC calls, and wait for further termination input.
    /// This will be a wrapper arouund setup() so we can change state if something fails inside
    pub async fn start(&mut self) -> Result<(), WayclipError> {
        tokio::spawn(MemoryManager::run_trim());
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

        if MemoryManager::memlog_enabled() {
            let ring = self.engine.ring();
            tokio::spawn(MemoryManager::run_memlog(Instant::now(), ring));
        }

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
                    self.schedule_recovery(&reason);
                }

                Some(SaveDone { result, responder }) = self.save_channel.1.recv() => {
                    self.status = DaemonStatus::Active;
                    tokio::task::spawn_blocking(MemoryManager::trim);
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
                    if self.status == DaemonStatus::Active {
                        self.track_health();
                        if self.recovery_due.is_some() {
                            self.run_due_recovery().await;
                        } else if self.engine.capture_degraded() {
                            self.schedule_recovery("guard dropped every frame");
                        } else if self.engine.is_stalled(STALL_LIMIT) {
                            log::warn!("no video frames for {}s", STALL_LIMIT.as_secs());
                            self.schedule_recovery("video stalled");
                        } else if self.engine.dmabuf_retry_due() {
                            self.retry_dmabuf().await;
                        }
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

    // ---  Recovery ---
    // TODO: MOVE TO SOME DIFF MODULE?

    /// Track how long video has been flowing
    fn track_health(&mut self) {
        if self.engine.video_flowing() {
            let since = *self.healthy_since.get_or_insert_with(Instant::now);
            if self.recovery_attempt > 0 && since.elapsed() >= HEALTHY_RESET {
                log::info!("video healthy again, resetting recovery ladder");
                self.recovery_attempt = 0;
            }
        } else {
            self.healthy_since = None;
        }
    }

    /// Ask for a recovery. This will never block or fail..
    fn schedule_recovery(&mut self, reason: &str) {
        if self.recovery_due.is_some() {
            return;
        }
        let delay = RECOVERY_BACKOFF[self.recovery_attempt.min(RECOVERY_BACKOFF.len() - 1)];
        log::warn!(
            "pipeline problem ({reason}); recovery #{} in {}s",
            self.recovery_attempt + 1,
            delay.as_secs()
        );
        self.recovery_due = Some((Instant::now() + delay, reason.to_string()));
    }

    fn is_video_memory_failure(reason: &str) -> bool {
        let reason = reason.to_ascii_lowercase();
        REASONS.iter().any(|needle| reason.contains(needle))
    }

    async fn run_due_recovery(&mut self) {
        let Some((_, reason)) = self.recovery_due.take_if(|(due, _)| Instant::now() >= *due) else {
            return;
        };

        let attempt = self.recovery_attempt;
        self.recovery_attempt += 1;

        let settings = self.current_session.user_settings.clone();
        let sender = self.event_channel.0.clone();
        let stalled = reason == "video stalled";
        let result = if !self.engine.has_portal() || attempt >= 2 || (stalled && attempt >= 1) {
            log::warn!("recovery #{}: full rebuild ({reason})", attempt + 1);
            if !stalled || attempt >= 2 {
                self.engine.capture_mode = CaptureMode::SystemMemory;
            }
            tokio::time::timeout(RECOVERY_TIMEOUT, self.engine.recover(&settings, sender)).await
        } else if attempt == 1 {
            log::warn!("recovery #2: rebuilding on system memory ({reason})");
            tokio::time::timeout(
                RECOVERY_TIMEOUT,
                self.engine
                    .downgrade_to_system_memory(&settings, sender, &reason),
            )
            .await
        } else if Self::is_video_memory_failure(&reason) {
            log::warn!("recovery #1: in-place restart ({reason})");
            tokio::time::timeout(RECOVERY_TIMEOUT, self.engine.retry(sender)).await
        } else {
            log::warn!("recovery #1: rebuilding pipeline ({reason})");
            tokio::time::timeout(
                RECOVERY_TIMEOUT,
                self.engine.restart_pipeline(&settings, sender),
            )
            .await
        };

        match result {
            Ok(Ok(())) => {
                while self.event_channel.1.try_recv().is_ok() {}
                self.healthy_since = None;
                log::info!("recovery #{} done", attempt + 1);
            }
            other => {
                log::error!(
                    "recovery #{} failed ({other:?}), will try again",
                    attempt + 1
                );
                self.schedule_recovery("previous recovery failed");
            }
        }
    }

    async fn retry_dmabuf(&mut self) {
        let settings = self.current_session.user_settings.clone();
        let sender = self.event_channel.0.clone();
        let res = tokio::time::timeout(
            RECOVERY_TIMEOUT,
            self.engine.retry_dmabuf(&settings, sender),
        )
        .await;

        match res {
            Ok(Ok(())) => while self.event_channel.1.try_recv().is_ok() {},
            other => {
                log::warn!("DMABuf retry failed to start ({other:?}). staying on system memory");
                self.engine.capture_mode = CaptureMode::SystemMemory;
                self.schedule_recovery("dmabuf retry failed");
            }
        }
    }
}
