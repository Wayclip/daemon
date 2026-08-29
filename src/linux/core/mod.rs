use crate::ShutdownReason;
use crate::common::discord::DiscordPresenceManager;
use crate::common::notifications::{NotificationEvent, NotificationManager};
use crate::common::ring::RingBuffer;
use crate::linux::core::types::{DaemonStatus, DefaultDeviceType, RecordingConfig};
use crate::linux::discovery::Discovery;
use crate::linux::pipewire::PipewireManager;
use ashpd::desktop::Session;
use ashpd::desktop::screencast::Screencast;
use gstreamer::ClockTime;
use gstreamer::prelude::ElementExt;
use nanoid::nanoid;
use sd_notify::NotifyState;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wayclip_core::models::error::WayclipError;
use wayclip_core::settings::discovery::GameDiscovery;
use wayclip_core::settings::notifications::NotificationSettings;
use wayclip_core::settings::output::OutputSettings;

pub(crate) mod actions;
pub(crate) mod audio;
pub(crate) mod mix;
pub(crate) mod screencast;
pub(crate) mod server;
pub(crate) mod types;
pub(crate) mod video;
pub(crate) mod watcher;

// const DEFAULT_ALLOW_MULTIPLE: bool = false;
// Yes this is a hack i found online
// changed it ACTUALLY be downstream now
pub(crate) const DEFAULT_APPSINK_MAX_BUFFERS: u32 = 100;
pub(crate) const DEFAULT_APPSINK_DROP: bool = false;
pub(crate) const DEFAULT_APPSINK_SYNC: bool = false;
pub(crate) const DEFAULT_PIPEWIRE_DO_TIMESTAMP: bool = true;
pub(crate) const DEFAULT_AUDIO_CHANNELS: i32 = 2;
pub(crate) const DEFAULT_PIPEWIRE_TIMEOUT: u64 = 4;

pub struct DaemonCore {
    id: String,
    generation: Arc<AtomicU64>,
    pub(crate) last_video_frame_time: Arc<AtomicU64>,
    status: DaemonStatus,
    discovery: Discovery,
    ring_buffer: Arc<Mutex<RingBuffer>>,
    notification_settings: NotificationSettings,
    restore_token: Option<String>,
    pipewire_manager: PipewireManager,
    gstreamer_pipeline: Option<gstreamer::Pipeline>,
    pipewire_proxy: Option<Screencast>,
    pipewire_session: Option<Session<Screencast>>,
    pipewire_file_descriptor: Option<OwnedFd>,
    pipewire_node_id: Option<String>,
    discord: Option<DiscordPresenceManager>,
    recording_config: RecordingConfig,
    output_config: OutputSettings,
}

impl DaemonCore {
    pub async fn init(
        daemon_arc: Arc<tokio::sync::Mutex<Self>>,
        config: RecordingConfig,
        discovery: GameDiscovery,
        cancel_token: CancellationToken,
        shutdown_sender: mpsc::Sender<ShutdownReason>,
    ) -> Result<(), WayclipError> {
        {
            let mut daemon = daemon_arc.lock().await;
            NotificationManager::process_event(
                NotificationEvent::DaemonStart,
                daemon.notification_settings.clone(),
                String::default(),
            )?;
            daemon.status = DaemonStatus::Activating;
        }

        gstreamer::init()?;

        let pipeline = Self::build_full_pipeline(daemon_arc.clone(), config).await?;

        let (generation, current_gen, last_video_frame_time) = {
            let daemon = daemon_arc.lock().await;
            (
                daemon.generation.clone(),
                daemon.generation.load(Ordering::Acquire),
                daemon.last_video_frame_time.clone(),
            )
        };

        Self::spawn_bus_watcher(
            daemon_arc.clone(),
            &pipeline,
            generation,
            last_video_frame_time,
            current_gen,
            cancel_token.clone(),
            shutdown_sender.clone(),
        )?;
        if discovery.enabled {
            Self::spawn_discovery_update(
                daemon_arc.clone(),
                discovery.poll_interval_s,
                cancel_token,
            )?;
        }

        {
            let mut daemon = daemon_arc.lock().await;
            daemon.status = DaemonStatus::Active;
            daemon.gstreamer_pipeline = Some(pipeline);
        }

        sd_notify::notify(&[NotifyState::Ready])?;

        Ok(())
    }

    pub async fn build_full_pipeline(
        daemon_arc: Arc<tokio::sync::Mutex<Self>>,
        mut config: RecordingConfig,
    ) -> Result<gstreamer::Pipeline, WayclipError> {
        let pipewire_manager = {
            let daemon = daemon_arc.lock().await;
            daemon.pipewire_manager.clone()
        };

        Self::check_audio_devices(&pipewire_manager, &mut config.audio).await?;

        let screencast = match Self::negotiate_screencast().await {
            Ok(negotiation) => negotiation,
            Err(e) => {
                let mut daemon = daemon_arc.lock().await;
                daemon.status = DaemonStatus::Failed;
                return Err(e);
            }
        };

        let pipeline = gstreamer::Pipeline::new();
        {
            let mut daemon = daemon_arc.lock().await;
            daemon.pipewire_session = Some(screencast.pipewire_session);
            daemon.pipewire_file_descriptor = Some(screencast.pipewire_file_descriptor);
            daemon.pipewire_node_id = Some(screencast.pipewire_node_id);
            daemon.pipewire_proxy = Some(screencast.pipewire_proxy);
            daemon.restore_token = screencast.restore_token;

            daemon.build_video_pipeline(
                &pipeline,
                config.codec,
                config.resolution.to_tuple(),
                config.fps.0,
                config.bitrate_kbps.0,
            )?;

            let mix = daemon.build_mix_pipeline(&pipeline, &config.audio)?;

            let sample_rate = &config.audio.sample_rate_hz;
            daemon.build_audio_source_pipeline(
                &pipeline,
                &config.audio.microphone,
                DefaultDeviceType::Microphone,
                sample_rate.0,
                &mix,
            )?;
            daemon.build_audio_source_pipeline(
                &pipeline,
                &config.audio.background,
                DefaultDeviceType::Background,
                sample_rate.0,
                &mix,
            )?;

            // Update all the 2 billion states
            pipeline.set_start_time(ClockTime::ZERO);
            pipeline.set_base_time(ClockTime::ZERO);

            if let Err(e) = pipeline.set_state(gstreamer::State::Playing) {
                let bus = pipeline
                    .bus()
                    .ok_or_else(|| WayclipError::Validation("No bus found".into()))?;
                let mut reason = "unknown".to_string();
                while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::from_mseconds(500)) {
                    if let gstreamer::MessageView::Error(err) = msg.view() {
                        reason = format!(
                            "{} ({:?}) from element {:?}",
                            err.error(),
                            err.debug(),
                            err.src().map(|s| s.to_string())
                        );
                        break;
                    }
                }
                let _ = pipeline.set_state(gstreamer::State::Null);
                daemon.status = DaemonStatus::Failed;
                return Err(WayclipError::Validation(
                    format!("set_state(Playing) failed synchronously ({e:?}): {reason}").into(),
                ));
            }
        }
        let pipeline_for_wait = pipeline.clone();
        let (state_result, current_state, _pending) = tokio::task::spawn_blocking(move || {
            pipeline_for_wait.state(gstreamer::ClockTime::from_seconds(10))
        })
        .await
        .map_err(|e| WayclipError::Validation(e.to_string().into()))?;

        if state_result.is_err() || current_state != gstreamer::State::Playing {
            // get the error
            let bus = pipeline
                .bus()
                .ok_or_else(|| WayclipError::Validation("No bus found".into()))?;
            let mut reason = "unknown".to_string();
            // this whole thing is same as for preview/saving
            while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::ZERO) {
                if let gstreamer::MessageView::Error(e) = msg.view() {
                    reason = format!("{} ({:?})", e.error(), e.debug());
                    break;
                }
            }

            let _ = pipeline.set_state(gstreamer::State::Null);
            let mut daemon = daemon_arc.lock().await;
            daemon.status = DaemonStatus::Failed;
            return Err(WayclipError::Validation(
                format!(
                    "Capture pipeline failed to reach PLAYING state ({:?}): {reason}",
                    state_result
                )
                .into(),
            ));
        }

        Ok(pipeline)
    }

    pub fn new(
        max_duration: u64,
        notification_settings: NotificationSettings,
        recording_config: RecordingConfig,
        output_config: OutputSettings,
        discord_rich_presence: bool,
    ) -> Result<Self, WayclipError> {
        Ok(Self {
            id: nanoid!(),
            discovery: Discovery::new()?,
            status: DaemonStatus::Inactive,
            generation: Arc::new(AtomicU64::new(1)),
            last_video_frame_time: Arc::new(AtomicU64::new(0)),
            ring_buffer: Arc::new(Mutex::new(RingBuffer::new(ClockTime::from_seconds(
                max_duration,
            )))),
            pipewire_manager: PipewireManager::new()?,
            restore_token: None,
            gstreamer_pipeline: None,
            pipewire_proxy: None,
            pipewire_node_id: None,
            pipewire_file_descriptor: None,
            pipewire_session: None,
            notification_settings,
            recording_config,
            output_config,
            discord: if discord_rich_presence {
                Some(DiscordPresenceManager::new())
            } else {
                None
            },
        })
    }
}
