use gstreamer::{
    self as gst, BufferFlags, Element, EventView, PadProbeData, PadProbeReturn, PadProbeType,
    prelude::*,
};
use gstreamer_video::{self as gst_video, VideoFormat, VideoInfoDmaDrm};
use parking_lot::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering},
};

const DEGRADED_AFTER_CONSECUTIVE_DROPS: u32 = 30;

/// So we we know whats happening in our pipeline
#[derive(Default, Debug)]
pub struct CaptureHealth {
    consecutive_dropped: AtomicU32,
    total_dropped: AtomicU64,
}

impl CaptureHealth {
    pub fn is_degraded(&self) -> bool {
        self.consecutive_dropped.load(Ordering::Relaxed) >= DEGRADED_AFTER_CONSECUTIVE_DROPS
    }

    pub fn total_dropped(&self) -> u64 {
        self.total_dropped.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.consecutive_dropped.store(0, Ordering::Relaxed);
    }

    fn good(&self) {
        if self.consecutive_dropped.load(Ordering::Relaxed) != 0 {
            self.consecutive_dropped.store(0, Ordering::Relaxed);
        }
    }

    fn bad(&self) {
        self.consecutive_dropped.fetch_add(1, Ordering::Relaxed);
        self.total_dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// Will get all frames and drop any corruped ones or ones that dont meet the caps
pub struct VideoBufferGuard;

impl VideoBufferGuard {
    pub fn attach(element: &Element, health: Arc<CaptureHealth>) {
        let Some(pad) = element.static_pad("sink") else {
            log::warn!("guard: {} has no sink pad, not guarding", element.name());
            return;
        };

        let expected: Arc<Mutex<Option<VideoFormat>>> = Arc::new(Mutex::new(None));

        pad.add_probe(
            PadProbeType::BUFFER | PadProbeType::EVENT_DOWNSTREAM,
            move |_, info| match &info.data {
                Some(PadProbeData::Event(event)) => {
                    if let EventView::Caps(c) = event.view() {
                        *expected.lock() = Self::format_of(c.caps());
                    }
                    PadProbeReturn::Ok
                }
                Some(PadProbeData::Buffer(buffer)) => {
                    let corrupt =
                        buffer.flags().contains(BufferFlags::CORRUPTED) || buffer.n_memory() == 0;

                    let mismatch = match (*expected.lock(), buffer.meta::<gst_video::VideoMeta>()) {
                        (Some(want), Some(meta)) => {
                            meta.format() != want && meta.format() != VideoFormat::DmaDrm
                        }
                        _ => false,
                    };

                    if corrupt || mismatch {
                        if health.total_dropped() % 60 == 0 {
                            log::warn!(
                                "dropping bad capture buffer (corrupt={corrupt} | caps format mismatch={mismatch} | total dropped={})",
                                health.total_dropped() + 1
                            );
                        }
                        health.bad();
                        PadProbeReturn::Drop
                    } else {
                        health.good();
                        PadProbeReturn::Ok
                    }
                }
                _ => PadProbeReturn::Ok,
            },
        );
    }

    fn format_of(caps: &gst::CapsRef) -> Option<VideoFormat> {
        let is_drm = caps
            .structure(0)?
            .get::<&str>("format")
            .is_ok_and(|f| f == "DMA_DRM");

        if is_drm {
            VideoInfoDmaDrm::from_caps(caps)
                .ok()?
                .to_video_info()
                .ok()
                .map(|i| i.format())
        } else {
            gst_video::VideoInfo::from_caps(caps)
                .ok()
                .map(|i| i.format())
        }
    }
}
