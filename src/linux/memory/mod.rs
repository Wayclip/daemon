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

const M_LOG_INTERVAL: Duration = Duration::from_secs(60);
const PR_SET_THP_DISABLE: i32 = 41;

unsafe extern "C" {
    fn mi_check_owned(p: *const std::ffi::c_void) -> bool;
}

pub fn allocator_is_mimalloc() -> bool {
    let probe: Vec<u8> = Vec::with_capacity(64);
    unsafe { mi_check_owned(probe.as_ptr() as *const std::ffi::c_void) }
}

pub struct MemoryManager;

impl MemoryManager {
    pub fn tune_allocator() {
        unsafe {
            libc::prctl(PR_SET_THP_DISABLE, 1, 0, 0, 0);
        }

        if !allocator_is_mimalloc() {
            log::error!(
                "mimalloc is linked but is NOT the global allocator. MemoryManager::trim is a no-op and trimmed memory will never be returned to the OS"
            );
        }
    }

    pub fn trim() {
        unsafe {
            libmimalloc_sys::mi_collect(true);
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

        let mut ticker = tokio::time::interval(M_LOG_INTERVAL);
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
