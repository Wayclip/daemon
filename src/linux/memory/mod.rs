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
const M_TRIM_THRESHOLD: i32 = -1;
const M_MMAP_THRESHOLD: i32 = -3;
const M_ARENA_MAX: i32 = -8;
const PR_SET_THP_DISABLE: i32 = 41;

pub struct MemoryManager;

impl MemoryManager {
    pub fn tune_allocator() {
        unsafe {
            libc::mallopt(M_ARENA_MAX, 2);
            libc::mallopt(M_MMAP_THRESHOLD, 16 * 1024);
            libc::mallopt(M_TRIM_THRESHOLD, 8 * 1024 * 1024);
            libc::prctl(PR_SET_THP_DISABLE, 1, 0, 0, 0);
        }
    }

    pub fn trim() {
        unsafe {
            libc::malloc_trim(0);
        }
    }

    pub async fn run_trim() {
        let mut t = tokio::time::interval(Duration::from_secs(30));
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            t.tick().await;
            let _ = tokio::task::spawn_blocking(Self::trim).await;
        }
    }

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
