use gstreamer::{Caps, ClockTime};
use gstreamer_app::AppSrc;
use wayclip_core::models::error::WayclipError;

use crate::common::video::ring::data::{ContentType, EncodedFrame};

pub const DEFAULT_APPSRC_FORMAT: gstreamer::Format = gstreamer::Format::Time;
pub const DEFAULT_APPSRC_IS_LIVE: bool = false;
pub const DEFAULT_APPSRC_DO_TIMESTAMP: bool = false;

pub struct GStreamerApp;

impl GStreamerApp {
    pub fn build_app_src(caps: &Caps) -> AppSrc {
        gstreamer_app::AppSrc::builder()
            .caps(caps)
            .format(DEFAULT_APPSRC_FORMAT)
            .is_live(DEFAULT_APPSRC_IS_LIVE)
            .do_timestamp(DEFAULT_APPSRC_DO_TIMESTAMP)
            .build()
    }

    fn sync_timestamp(ts: ClockTime, base: ClockTime, offset_ms: i64) -> ClockTime {
        let normalised = ts.saturating_sub(base);
        let offset_dur = ClockTime::from_mseconds(offset_ms.unsigned_abs());
        if offset_ms >= 0 {
            normalised.saturating_add(offset_dur)
        } else {
            normalised.saturating_sub(offset_dur)
        }
    }

    pub fn push_frames(
        appsrc: AppSrc,
        frame_info: EncodedFrameInfo,
    ) -> std::thread::JoinHandle<Result<(), WayclipError>> {
        std::thread::spawn(move || {
            for mut frame in frame_info.frames {
                let buffer_ref = frame.payload.make_mut();

                let pts = GStreamerApp::sync_timestamp(
                    frame.pts,
                    frame_info.base_pts,
                    frame_info.offset_ms,
                );
                buffer_ref.set_pts(pts);

                if let Some(dts) = frame.dts {
                    let dts = GStreamerApp::sync_timestamp(
                        dts,
                        frame_info.base_pts,
                        frame_info.offset_ms,
                    );
                    buffer_ref.set_dts(dts);
                }

                if let Some(duration) = frame.duration {
                    buffer_ref.set_duration(duration);
                }

                if !frame.is_keyframe {
                    buffer_ref.set_flags(gstreamer::BufferFlags::DELTA_UNIT);
                }

                appsrc.push_buffer(frame.payload).map_err(|e| {
                    WayclipError::Remux(format!("Failed to push buffer: {:?}", e).into())
                })?;
            }

            appsrc
                .end_of_stream()
                .map_err(|e| WayclipError::Remux(format!("Failed to send EOS: {:?}", e).into()))?;

            Ok(())
        })
    }
}

pub struct EncodedFrameInfo {
    pub frames: Vec<EncodedFrame>,
    pub content_type: ContentType,
    pub base_pts: ClockTime,
    pub offset_ms: i64,
}
