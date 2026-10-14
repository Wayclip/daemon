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

#[derive(Clone, Copy, Default)]
struct Expected {
    format: Option<VideoFormat>,
    width: u32,
    height: u32,
    dmabuf: bool,
}

/// Basically, this is to fifnd faulty frames and drop them instead of causing a fatal error.
pub struct VideoBufferGuard;

impl VideoBufferGuard {
    pub fn attach(element: &Element, health: Arc<CaptureHealth>) {
        let Some(pad) = element.static_pad("sink") else {
            log::warn!("guard: {} has no sink pad, not guarding", element.name());
            return;
        };

        let expected: Arc<Mutex<Expected>> = Arc::new(Mutex::new(Expected::default()));

        pad.add_probe(
            PadProbeType::BUFFER | PadProbeType::EVENT_DOWNSTREAM,
            move |_, info| match &info.data {
                Some(PadProbeData::Event(event)) => {
                    if let EventView::Caps(c) = event.view() {
                        *expected.lock() = Self::expected_from(c.caps());
                    }
                    PadProbeReturn::Ok
                }
                Some(PadProbeData::Buffer(buffer)) => {
                    let want = *expected.lock();
                    let n_mem = buffer.n_memory();
                    let meta = buffer.meta::<gst_video::VideoMeta>();

                    let corrupt = buffer.flags().contains(BufferFlags::CORRUPTED) || n_mem == 0;

                    let not_dmabuf =
                        want.dmabuf && n_mem > 0 && !buffer.peek_memory(0).is_type("dmabuf");

                    let mut mismatch = false;
                    let mut too_many_mem = false;
                    if let Some(meta) = meta.as_ref() {
                        mismatch = want
                            .format
                            .is_some_and(|f| meta.format() != f && meta.format() != VideoFormat::DmaDrm)
                            || (want.width != 0 && meta.width() < want.width)
                            || (want.height != 0 && meta.height() < want.height);
                        too_many_mem = (n_mem as u32) > meta.n_planes();
                    }

                    if corrupt || not_dmabuf || mismatch || too_many_mem {
                        if health.total_dropped() % 60 == 0 {
                            log::warn!(
                                "dropping bad capture buffer. corrupt={corrupt} not_dmabuf={not_dmabuf} meta_mismatch={mismatch} too_many_mem={too_many_mem} total dropped={}",
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

    fn expected_from(caps: &gst::CapsRef) -> Expected {
        let dmabuf = caps
            .features(0)
            .is_some_and(|f| f.contains("memory:DMABuf"));
        let (width, height) = caps
            .structure(0)
            .map(|s| {
                (
                    s.get::<i32>("width").unwrap_or(0).max(0) as u32,
                    s.get::<i32>("height").unwrap_or(0).max(0) as u32,
                )
            })
            .unwrap_or((0, 0));

        Expected {
            format: Self::format_of(caps),
            width,
            height,
            dmabuf,
        }
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
