use crate::{
    common::gst::guard::CaptureHealth,
    common::video::ring::RingBuffer,
    linux::memory::{
        memlog::{Counters, Sample},
        nvidia::Nvidia,
    },
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

pub mod allocator_env;
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
        allocator_env::AllocatorEnv::ensure();
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

    pub async fn run_memlog(
        started: Instant,
        ring: Arc<Mutex<RingBuffer>>,
        health: Arc<CaptureHealth>,
        target_fps: f64,
    ) {
        if !Self::memlog_enabled() {
            return;
        }

        let mut ticker = tokio::time::interval(M_LOG_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut prev = Counters::default();
        let mut low_streak: u32 = 0;

        loop {
            ticker.tick().await;
            let sample = Sample::capture(
                started,
                &ring,
                &mut prev,
                target_fps,
                health.total_dropped(),
            );
            log::debug!("{}", sample);

            if sample.video_flowing {
                low_streak = 0;
            } else {
                low_streak += 1;
                if low_streak == 2 {
                    log::warn!(
                        "capture chain has been under target for 2 intervals: {:.2} fps, target {:.0}, ring holds {} frames, {} buffers dropped total",
                        sample.video_fps,
                        target_fps,
                        sample.video_frames,
                        sample.dropped,
                    );
                }
            }
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
