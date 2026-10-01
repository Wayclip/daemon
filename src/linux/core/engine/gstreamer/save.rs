use chrono::Local;
use parking_lot::Mutex;
use std::sync::Arc;
use wayclip_core::{
    app::clips::query::ClipsQuery,
    models::{clips::local::LocalClip, error::WayclipError},
};

use crate::{
    PreviewManager,
    common::video::{SaveManager, ring::RingBuffer},
    linux::core::session::CurrentSession,
};

pub struct SavePipelineFactory;

const DEFAULT_MIN_FRAMES_FOR_SAVE: usize = 100;

impl SavePipelineFactory {
    pub async fn save(
        current_session: &CurrentSession,
        forced_name: Option<String>,
        ring: Arc<Mutex<RingBuffer>>,
    ) -> Result<(), WayclipError> {
        let user_settings = &current_session.user_settings;
        let (saved_data, frame_count) = {
            let ring = ring.lock();
            (ring.get_snapshot()?, ring.video_frames.len())
        };

        if frame_count < DEFAULT_MIN_FRAMES_FOR_SAVE {
            let message = format!(
                "Cannot save clip: Not enough frames in buffer yet ({}/{} frames). Stream might be lagging.",
                frame_count, DEFAULT_MIN_FRAMES_FOR_SAVE
            );
            log::warn!("{}", message);
            return Err(WayclipError::Ring(message.into()));
        }

        let (video_bytes, audio_bytes) = saved_data.get_bytes();

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
        if user_settings.output.limit.max_size_mb != 0
            && total_size_mb + predicted_size_mb > user_settings.output.limit.max_size_mb
        {
            let message = format!(
                "Cannot save clip: Predicted size ({} MB) exceeds user limit ({} MB).",
                predicted_size_mb, user_settings.output.limit.max_size_mb
            );
            log::warn!("{}", message);
            return Err(WayclipError::Ring(message.into()));
        }

        // 0 means unbounded
        if user_settings.output.limit.max_clips != 0
            && total_clip_num + 1 > user_settings.output.limit.max_clips as usize
        {
            let message = format!(
                "Cannot save clip: Total clip number exceeds user limit ({} clips).",
                user_settings.output.limit.max_clips
            );
            log::warn!("{}", message);
            return Err(WayclipError::Ring(message.into()));
        }

        let duration = saved_data.duration;

        // TODO: CUSTOM FORMATTING LIKE W DISCORD STATUS
        let mut parsed_name = Local::now()
            .format(&user_settings.output.name_format)
            .to_string();
        let game_str = match current_session.game {
            None => "desktop",
            Some(g) => g.slug(),
        };
        parsed_name = parsed_name.replace("{game}", game_str);

        log::debug!("Formatted clip name: {}", parsed_name);

        let clip_name = match forced_name {
            None => format!(
                "{}.{}",
                parsed_name,
                user_settings.output.video_format.get_extension()
            ),
            Some(n) => n,
        };

        let clip_output_path = user_settings.output.clip_directory.0.join(&clip_name);

        let preview_name = format!("{}.preview.mkv", parsed_name);
        let preview_output_path = user_settings.output.preview_directory.0.join(preview_name);

        let metadata_name = format!("{}.json", parsed_name);
        let metadata_output_path = user_settings
            .output
            .metadata_directory
            .0
            .join(metadata_name);

        log::debug!(
            "Spawning tokio task to write video to {}",
            clip_output_path.to_string_lossy()
        );

        let clip_path_for_remux = clip_output_path.clone();
        let preview_path_for_remux = preview_output_path.clone();
        let format_for_remux = user_settings.output.video_format.clone();

        // Yes okay i have a stroke reading this aswell
        let handle = tokio::task::spawn_blocking(move || -> Result<(), WayclipError> {
            // Apparently if ur using ::default, you may not even initialise it.
            SaveManager::save_clip(saved_data, format_for_remux, &clip_path_for_remux)?;

            std::thread::spawn(move || {
                match PreviewManager::generate_preview(
                    clip_path_for_remux.as_ref(),
                    preview_path_for_remux.as_ref(),
                ) {
                    Err(e) => log::error!("Could not generate preview {e}"),
                    Ok(_) => log::info!("Preview successfully generated"),
                };
            });

            Ok(())
        });

        async {
            handle
                .await
                .map_err(|e| WayclipError::Validation(e.to_string().into()))??;

            LocalClip::new(
                &parsed_name,
                user_settings.output.video_format.clone(),
                clip_output_path,
                preview_output_path,
                metadata_output_path,
                current_session.game,
                Some(duration.mseconds()),
                user_settings.recording.video.bitrate_kbps.clone(),
                user_settings.recording.video.resolution.clone(),
                user_settings.recording.video.fps.clone(),
            )
            .await?;

            Ok(())
        }
        .await
    }
}
