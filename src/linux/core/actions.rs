use crate::common::notifications::{NotificationEvent, NotificationManager};
use crate::common::remux::RemuxHandler;
use crate::linux::core::DaemonCore;
use crate::linux::core::types::DaemonStatus;
use chrono::Local;
use gstreamer::prelude::ElementExt;
use log::{error, info, warn};
use sd_notify::NotifyState;
use std::sync::Arc;
use wayclip_core::app::clips::query::ClipsQuery;
use wayclip_core::models::clips::local::LocalClip;
use wayclip_core::models::error::WayclipError;

const DEFAULT_MIN_FRAMES_FOR_SAVE: usize = 100;

impl DaemonCore {
    pub async fn rescan_games(&mut self) -> (String, f32) {
        self.discovery.discover_game();
        let game = self
            .discovery
            .confident_game()
            .map(|g| g.to_string())
            .unwrap_or_default();
        let confidence = self.discovery.confidence;
        (game, confidence)
    }

    pub async fn shutdown(&mut self) -> Result<(), WayclipError> {
        info!("Initiating graceful shutdown");

        self.status = DaemonStatus::Deactivating;

        if let Some(pipeline) = self.gstreamer_pipeline.as_ref() {
            let _ = pipeline.set_state(gstreamer::State::Null);
        }

        self.gstreamer_pipeline = None;

        self.pipewire_file_descriptor = None;
        self.pipewire_proxy = None;
        self.pipewire_node_id = None;
        if let Some(session) = self.pipewire_session.take()
            && let Err(e) = session.close().await
        {
            warn!("Failed to close portal session on shutdown: {e}");
        }

        NotificationManager::process_event(
            NotificationEvent::DaemonStop,
            self.notification_settings.clone(),
            String::default(),
        )?;

        sd_notify::notify(&[NotifyState::Stopping])?;
        self.status = DaemonStatus::Inactive;

        info!("Graceful shutdown complete");

        Ok(())
    }

    pub async fn get_status(&self) -> Result<DaemonStatus, WayclipError> {
        info!("Recieved a get status call");
        Ok(self.status.clone())
    }

    pub async fn save_clip(
        core: Arc<tokio::sync::Mutex<Self>>,
        forced_name: Option<String>,
    ) -> Result<(), WayclipError> {
        let (
            saved_data,
            first_video_pts,
            output_config,
            notification_settings,
            discovery,
            recording_config,
        ) = {
            let this = core.lock().await;
            info!("Recieved a save clip call");

            if this.status == DaemonStatus::Saving {
                warn!("Save already in progress...");
                return Ok(());
            }

            let (first_video_pts, saved_data, frame_count) = {
                let ring_buffer = this
                    .ring_buffer
                    .lock()
                    .map_err(|e| WayclipError::Validation(e.to_string().into()))?;
                (
                    ring_buffer.video_first_pts,
                    ring_buffer.get_snapshot()?,
                    ring_buffer.video_frames.len(),
                )
            };

            if frame_count < DEFAULT_MIN_FRAMES_FOR_SAVE {
                let message = format!(
                    "Cannot save clip: Not enough frames in buffer yet ({}/{} frames). Stream might be lagging.",
                    frame_count, DEFAULT_MIN_FRAMES_FOR_SAVE
                );
                warn!("{}", message);
                NotificationManager::process_event(
                    NotificationEvent::SaveError,
                    this.notification_settings.clone(),
                    message,
                )?;
                return Ok(());
            }

            (
                saved_data,
                first_video_pts,
                this.output_config.clone(),
                this.notification_settings.clone(),
                this.discovery.clone(),
                this.recording_config.clone(),
            )
        };

        // predicting file size
        let video_bytes = saved_data
            .video_frames
            .iter()
            .map(|f| f.data.size() as u64)
            .sum::<u64>();
        let audio_bytes = saved_data
            .audio_frames
            .iter()
            .map(|f| f.data.size() as u64)
            .sum::<u64>();

        // add 1.5%
        let predicted_bytes = ((video_bytes + audio_bytes) as f64 * 1.015) as u64;
        let predicted_size_mb = predicted_bytes / 1000000;

        // didnt wanna do this, but its better and more consitant
        let all_clips = ClipsQuery::get_all_local_clips().await?;
        let total_clip_num = all_clips.len();
        let total_size_mb = all_clips
            .clone()
            .iter()
            .map(|c| c.file_size_mb)
            .sum::<u64>();

        // 0 means unbounded
        if output_config.limit.max_size_mb != 0
            && total_size_mb + predicted_size_mb > output_config.limit.max_size_mb
        {
            let message = format!(
                "Cannot save clip: Predicted size ({} MB) exceeds user limit ({} MB).",
                predicted_size_mb, output_config.limit.max_size_mb
            );
            warn!("{}", message);
            NotificationManager::process_event(
                NotificationEvent::SaveError,
                notification_settings,
                message,
            )?;
            return Ok(());
        }

        // 0 means unbounded
        if output_config.limit.max_clips != 0
            && total_clip_num + 1 > output_config.limit.max_clips as usize
        {
            let message = format!(
                "Cannot save clip: Total clip number exceeds user limit ({} clips).",
                output_config.limit.max_clips
            );
            warn!("{}", message);
            NotificationManager::process_event(
                NotificationEvent::SaveError,
                notification_settings,
                message,
            )?;
            return Ok(());
        }

        {
            let mut this = core.lock().await;
            if this.status == DaemonStatus::Saving {
                warn!("Save already in progress...");
                return Ok(());
            }
            this.status = DaemonStatus::Saving;
        }

        let duration = saved_data.duration;

        let mut parsed_name = Local::now().format(&output_config.name_format).to_string();
        // use the confident version
        let game_str = match discovery.confident_game() {
            None => "desktop",
            Some(g) => g.slug(),
        };
        parsed_name = parsed_name.replace("{game}", game_str);

        log::debug!("Formatted clip name: {}", parsed_name);

        let clip_name = match forced_name {
            None => format!(
                "{}.{}",
                parsed_name,
                output_config.video_format.get_extension()
            ),
            Some(n) => n,
        };

        let clip_output_path = output_config.clip_directory.0.join(&clip_name);

        let preview_name = format!("{}.preview.mkv", parsed_name);
        let preview_output_path = output_config.preview_directory.0.join(preview_name);

        // TODO: Add its own dir, for now its in same dir as clip
        let metadata_name = format!("{}.json", parsed_name);
        let metadata_output_path = output_config.clip_directory.0.join(metadata_name);

        log::debug!(
            "Spawning tokio task to write video to {}",
            clip_output_path.to_string_lossy()
        );

        let clip_path_for_remux = clip_output_path.clone();
        let preview_path_for_remux = preview_output_path.clone();
        let format_for_remux = output_config.video_format.clone();

        // Yes okay i have a stroke reading this aswell
        let handle = tokio::task::spawn_blocking(move || -> Result<(), WayclipError> {
            // Apparently if ur using ::default, you may not even initialise it.
            RemuxHandler.run_remux_pipeline(
                saved_data,
                format_for_remux,
                clip_path_for_remux.clone(),
            )?;

            std::thread::spawn(move || {
                match RemuxHandler.generate_preview(
                    clip_path_for_remux,
                    preview_path_for_remux,
                    first_video_pts,
                ) {
                    Err(e) => error!("Could not generate preview {e}"),
                    Ok(_) => info!("Preview successfully generated"),
                };
            });

            Ok(())
        });

        let result: Result<(), WayclipError> = async {
            handle
                .await
                .map_err(|e| WayclipError::Validation(e.to_string().into()))??;

            LocalClip::new(
                &parsed_name,
                output_config.video_format,
                clip_output_path,
                preview_output_path,
                metadata_output_path,
                discovery.confident_game(),
                Some(duration.mseconds()),
                recording_config.bitrate_kbps,
                recording_config.resolution,
                recording_config.fps,
            )
            .await?;

            Ok(())
        }
        .await;

        match result {
            Ok(_) => {
                info!("Clip saved succesfully");
                NotificationManager::process_event(
                    NotificationEvent::SaveSuccess,
                    notification_settings,
                    clip_name,
                )?;
                let mut this = core.lock().await;
                this.status = DaemonStatus::Active;
            }
            Err(e) => {
                error!("Failed to save clip {}", e);
                NotificationManager::process_event(
                    NotificationEvent::SaveError,
                    notification_settings,
                    e.to_string(),
                )?;
                let mut this = core.lock().await;
                this.status = DaemonStatus::Failed;
                return Err(e);
            }
        };

        Ok(())
    }
}
