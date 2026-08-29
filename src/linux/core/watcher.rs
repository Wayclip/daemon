use crate::ShutdownReason;
use crate::linux::core::DaemonCore;
use crate::linux::core::types::DaemonStatus;
use gstreamer::prelude::ElementExt;
use log::{error, warn};
use nanoid::nanoid;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use wayclip_core::models::error::WayclipError;

const PIPELINE_STALL_S: u64 = 10;

impl DaemonCore {
    pub fn spawn_discovery_update(
        daemon_arc: Arc<tokio::sync::Mutex<Self>>,
        poll_interval_s: u64,
        cancel_token: CancellationToken,
    ) -> Result<(), WayclipError> {
        tokio::spawn(async move {
            let mut timer = interval(Duration::from_secs(poll_interval_s));

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        log::debug!("Discovery update loop shutting down");
                        break;
                    }
                    _ = timer.tick() => {
                        let mut lock = daemon_arc.lock().await;
                        lock.discovery.discover_game();
                        let game_name = lock.discovery.confident_game().map(|g| g.to_string());
                        log::debug!("(discord) confident game: {game_name:?}");
                        if let Some(discord) = &lock.discord {
                            discord.set_recording(game_name);
                        }
                    }
                }
            }
        });

        Ok(())
    }

    pub fn spawn_bus_watcher(
        daemon_arc: Arc<tokio::sync::Mutex<Self>>,
        pipeline: &gstreamer::Pipeline,
        generation: Arc<AtomicU64>,
        last_video_frame_time: Arc<AtomicU64>,
        my_generation: u64,
        cancel_token: CancellationToken,
        shutdown_sender: mpsc::Sender<ShutdownReason>,
    ) -> Result<(), WayclipError> {
        // this will always check if our pipeline died, otherwise any erorrs are silent
        let bus = pipeline
            .bus()
            .ok_or_else(|| WayclipError::Watcher("Pipeline has no bus".into()))?;
        let start_time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        tokio::task::spawn_blocking(move || {
            log::debug!("Bus watcher active");

            while !cancel_token.is_cancelled() {
                if generation.load(Ordering::Acquire) != my_generation {
                    log::info!("Pipeline generation {my_generation} is stale, watcher exiting.");
                    return;
                }

                if let Some(message) = bus.timed_pop(gstreamer::ClockTime::from_seconds(1)) {
                    match message.view() {
                        gstreamer::MessageView::Error(e) => {
                            let msg = format!("{} ({:?})", e.error(), e.debug());
                            error!("Capture pipeline error: {msg}");

                            // format regonitiation error (idk why it happens)
                            let recoverable = msg.contains("unhandled format")
                                || msg.contains("pipewiresrc")
                                || msg.contains("negotiation")
                                || msg.contains("not-negotiated")
                                || msg.contains("stream error")
                                || msg.contains("No data received")
                                || msg.contains("on_state_changed")
                                || e.error().is::<gstreamer::StreamError>()
                                || e.error().is::<gstreamer::ResourceError>();

                            if recoverable {
                                log::warn!("Recoverable error recieved, attempting to restart");
                                let daemon_arc = daemon_arc.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(Duration::from_millis(500)).await;
                                    if let Err(e) = DaemonCore::recover_pipeline(
                                        daemon_arc,
                                        cancel_token.clone(),
                                        shutdown_sender.clone(),
                                    )
                                    .await
                                    {
                                        shutdown_sender
                                            .send(ShutdownReason::FatalError(format!(
                                                "Pipeline recovery failed: {e}"
                                            )))
                                            .await
                                            .ok();
                                        cancel_token.cancel();
                                    }
                                });
                            } else {
                                let shutdown_sender = shutdown_sender.clone();
                                let cancel_token = cancel_token.clone();
                                tokio::spawn(async move {
                                    shutdown_sender
                                        .send(ShutdownReason::FatalError(
                                            "Fatal error, exiting.".into(),
                                        ))
                                        .await
                                        .ok();
                                    cancel_token.cancel();
                                });
                            }

                            break;
                        }
                        gstreamer::MessageView::Eos(_) => {
                            let shutdown_sender = shutdown_sender.clone();
                            let cancel_token = cancel_token.clone();
                            tokio::spawn(async move {
                                shutdown_sender
                                    .send(ShutdownReason::FatalError(
                                        "Capture pipeline unexpectedly hit EOS".into(),
                                    ))
                                    .await
                                    .ok();
                                cancel_token.cancel();
                            });
                            break;
                        }
                        gstreamer::MessageView::Warning(w) => {
                            warn!("Capture pipeline warning: {:?}", w);
                        }
                        _ => {}
                    }
                } else {
                    let now_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;

                    let last_frame = last_video_frame_time.load(Ordering::Acquire);
                    let startup_stall = last_frame == 0 && (now_ms - start_time_ms > 5000);

                    let runtime_stall = last_frame > 0
                        && (now_ms.saturating_sub(last_frame) > PIPELINE_STALL_S * 1000);

                    if startup_stall || runtime_stall {
                        log::warn!(
                            "Video stream stall detected (no frames for {}ms). Triggering recovery...",
                            if last_frame == 0 {
                                now_ms - start_time_ms
                            } else {
                                now_ms - last_frame
                            }
                        );

                        let daemon_arc = daemon_arc.clone();
                        let cancel_token = cancel_token.clone();
                        let shutdown_sender = shutdown_sender.clone();

                        tokio::spawn(async move {
                            if let Err(e) = DaemonCore::recover_pipeline(
                                daemon_arc,
                                cancel_token.clone(),
                                shutdown_sender.clone(),
                            )
                            .await
                            {
                                shutdown_sender
                                    .send(ShutdownReason::FatalError(format!(
                                        "Pipeline recovery failed: {e}"
                                    )))
                                    .await
                                    .ok();
                                cancel_token.cancel();
                            }
                        });

                        break;
                    }
                }
            }

            log::debug!("Bus watcher thread exiting");
        });

        Ok(())
    }

    pub async fn recover_pipeline(
        daemon_arc: Arc<tokio::sync::Mutex<Self>>,
        cancel_token: CancellationToken,
        shutdown_sender: mpsc::Sender<ShutdownReason>,
    ) -> Result<(), WayclipError> {
        let (config, old_pipeline, generation, next_gen, last_video_frame_time) = {
            let core = daemon_arc.lock().await;
            let next_gen = core.generation.fetch_add(1, Ordering::SeqCst) + 1;
            core.last_video_frame_time.store(0, Ordering::Release);
            (
                core.recording_config.clone(),
                core.gstreamer_pipeline.clone(),
                core.generation.clone(),
                next_gen,
                core.last_video_frame_time.clone(),
            )
        };

        {
            let core = daemon_arc.lock().await;
            let mut ring_buffer = core.ring_buffer.lock().map_err(|e| {
                WayclipError::Watcher(format!("Could not acquire lock: {e}").into())
            })?;
            ring_buffer.begin_resync();
        }

        // A cleaner way to flush fully the old pipeline
        if let Some(old) = old_pipeline {
            log::info!("Tearing down old pipeline");

            old.set_state(gstreamer::State::Null).ok();

            let mut reached = false;
            for attempt in 1..=5 {
                if let Some(bus) = old.bus() {
                    while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::from_mseconds(50)) {
                        log::trace!("Teardown bus message drained: {:?}", msg.type_());
                    }
                }

                if let (Ok(_), cur, _) = old.state(gstreamer::ClockTime::from_seconds(1))
                    && cur == gstreamer::State::Null
                {
                    reached = true;
                    log::info!(
                        "Old pipeline reached NULL successfully on attempt {}",
                        attempt
                    );
                    break;
                }

                std::thread::sleep(Duration::from_millis(100));
            }

            if !reached {
                log::warn!(
                    "Old pipeline failed to reach NULL cleanly after multiple attempts; forcing teardown"
                );
            }

            if let Some(bus) = old.bus() {
                bus.set_flushing(true);
            }
        }

        {
            let mut core = daemon_arc.lock().await;
            if let Some(session) = core.pipewire_session.take()
                && let Err(e) = session.close().await
            {
                log::warn!("Failed to close stale portal session: {e}");
            }
            core.pipewire_proxy = None;
            core.pipewire_node_id = None;
            core.pipewire_file_descriptor = None;
            core.id = nanoid!();
        }

        let new_pipeline = Self::build_full_pipeline(daemon_arc.clone(), config).await?;

        Self::spawn_bus_watcher(
            daemon_arc.clone(),
            &new_pipeline,
            generation,
            last_video_frame_time,
            next_gen,
            cancel_token,
            shutdown_sender.clone(),
        )?;

        {
            let mut core = daemon_arc.lock().await;
            core.gstreamer_pipeline = Some(new_pipeline);
            core.status = DaemonStatus::Active;
        }

        log::info!("Pipeline recovery success. Data preserved");

        Ok(())
    }
}
