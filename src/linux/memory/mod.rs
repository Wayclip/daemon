use crate::{
    common::video::ring::RingBuffer,
    linux::memory::{memlog::Sample, nvidia::Nvidia},
};
use parking_lot::Mutex;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use wayclip_core::{
    models::error::WayclipError,
    settings::{UserSettings, recording::CodecType},
};

pub mod memlog;
pub mod nvidia;

pub const MEMORY_LOG_INTERVAL: Duration = Duration::from_secs(60);

pub struct MemoryManager;

impl MemoryManager {
    pub fn memlog_enabled() -> bool {
        log::log_enabled!(log::Level::Debug)
    }

    pub async fn run_memlog(started: Instant, ring: Arc<Mutex<RingBuffer>>) {
        if !Self::memlog_enabled() {
            return;
        }

        let mut ticker = tokio::time::interval(MEMORY_LOG_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;
            log::debug!("{}", Sample::capture(started, &ring));
        }
    }

    /// Will only exec if NVIDIA selected. Will apply profile + restart daemon w flag
    pub fn reserve_video_memory() -> Result<(), WayclipError> {
        let settings = UserSettings::load()?;
        match *settings.recording.video.codec.get_backend() {
            CodecType::NVIDIA => {
                let changed = Nvidia::ensure_vram_profile()?;
                Nvidia::reexec(changed)
            }
            _ => return Ok(()),
        }
    }
}
