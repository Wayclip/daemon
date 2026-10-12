use gstreamer::ClockTime;
use parking_lot::Mutex;
use std::fmt::Write;
use std::{path::Path, sync::Arc};
use tokio::sync::oneshot;
use wayclip_core::{
    app::clips::query::ClipsQuery,
    models::{clips::local::LocalClip, error::WayclipError},
};

use crate::{
    common::video::{SaveManager, preview::PreviewManager, ring::RingBuffer},
    linux::session::CurrentSession,
};

pub struct SaveDone {
    pub result: Result<String, WayclipError>,
    pub responder: oneshot::Sender<Result<String, WayclipError>>,
}

pub struct SavePipelineFactory;

impl SavePipelineFactory {
    pub async fn save(
        current_session: &CurrentSession,
        forced_name: Option<String>,
        ring: Arc<Mutex<RingBuffer>>,
    ) -> Result<String, WayclipError> {
        let user_settings = &current_session.user_settings;
        let saved_data = {
            let ring = ring.lock();
            ring.get_snapshot()?
        };

        let min =
            ClockTime::from_seconds(2).min(user_settings.recording.video.get_max_duration() / 2);
        if saved_data.duration < min || saved_data.video_frames.len() < 2 {
            let message = format!(
                "Cannot save clip: Not enough frames in buffer yet. Stream might be lagging.",
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

        let ext = user_settings
            .output
            .video_format
            .get_extension()
            .to_string();
        let game_str = current_session
            .game
            .as_ref()
            .map(|g| g.slug.clone())
            .unwrap_or_else(|| "desktop".to_string());

        let base = match forced_name {
            Some(n) => Self::safe_stem(&n, &ext),
            None => Self::safe_stem(
                &Self::format_name(&user_settings.output.name_format).replace("{game}", &game_str),
                &ext,
            ),
        };
        let clip_dir = &user_settings.output.clip_directory.0;
        let stem = Self::unique_stem(clip_dir, &base, &ext);

        let clip_output_path = clip_dir.join(format!("{stem}.{ext}"));
        let preview_output_path = user_settings
            .output
            .preview_directory
            .0
            .join(format!("{stem}.preview.mkv"));
        let metadata_output_path = user_settings
            .output
            .metadata_directory
            .0
            .join(format!("{stem}.json"));

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
                &stem,
                user_settings.output.video_format.clone(),
                clip_output_path,
                preview_output_path,
                metadata_output_path,
                current_session.game.clone(),
                Some(duration.mseconds()),
                user_settings.recording.video.bitrate_kbps.clone(),
                user_settings.recording.video.resolution.clone(),
                user_settings.recording.video.fps.clone(),
            )
            .await?;

            Ok::<(), WayclipError>(())
        }
        .await?;

        Ok(stem)
    }

    fn safe_stem(raw: &str, ext: &str) -> String {
        let name = Path::new(raw.trim())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let name = name.strip_suffix(&format!(".{ext}")).unwrap_or(name);
        let cleaned: String = name
            .chars()
            .map(|c| {
                // char::from(92) is `\`
                if c.is_control()
                    || c == char::from(92)
                    || matches!(c, '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
                {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let cleaned: String = cleaned.trim().trim_matches('.').chars().take(120).collect();
        if cleaned.is_empty() {
            "clip".into()
        } else {
            cleaned
        }
    }

    fn unique_stem(dir: &Path, stem: &str, ext: &str) -> String {
        let (mut candidate, mut n) = (stem.to_string(), 1);
        while dir.join(format!("{candidate}.{ext}")).exists() {
            candidate = format!("{stem}-{n}");
            n += 1;
        }
        candidate
    }

    // TODO: Add more cool options to format like w discord
    fn format_name(fmt: &str) -> String {
        let now = chrono::Local::now();
        let mut out = String::new();
        if write!(out, "{}", now.format(fmt)).is_err() {
            log::warn!("invalid output.name_format {:?}, using default", fmt);
            out = now.format("%Y-%m-%d_%H-%M-%S").to_string();
        }
        out
    }
}
