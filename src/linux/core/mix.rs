use crate::common::ring::{ContentType, EncodedFrame};
use crate::linux::core::{
    DEFAULT_APPSINK_DROP, DEFAULT_APPSINK_MAX_BUFFERS, DEFAULT_APPSINK_SYNC,
    DEFAULT_AUDIO_CHANNELS, DaemonCore,
};
use gstreamer::ClockTime;
use gstreamer::glib::object::Cast;
use gstreamer::prelude::{ElementExtManual, GstBinExt, ObjectExt};
use log::error;
use std::sync::atomic::Ordering;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use wayclip_core::models::error::WayclipError;
use wayclip_core::settings::recording::AudioSettings;

impl DaemonCore {
    pub fn set_appsink_callbacks(
        &self,
        appsink: &gstreamer_app::AppSink,
        content_type: ContentType,
    ) -> Result<(), WayclipError> {
        let ring_buffer_clone = self.ring_buffer.clone();
        let last_video_frame_time = self.last_video_frame_time.clone();
        let last_audio_frame_time = self.last_audio_frame_time.clone();
        appsink.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    match ring_buffer_clone.lock() {
                        Ok(mut ring_buffer) => {
                            let sample =
                                sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                            let buffer_ref = sample.buffer().ok_or(gstreamer::FlowError::Error)?;

                            let caps = sample.caps().map(|caps| caps.to_owned());
                            let mut pts = buffer_ref.pts().ok_or_else(|| {
                                error!("{} buffer missing PTS", content_type);
                                gstreamer::FlowError::Error
                            })?;

                            match content_type {
                                ContentType::Video => {
                                    let now_ms = SystemTime::now()

                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis() as u64;
                                    last_video_frame_time.store(now_ms, Ordering::Release);
                                    if ring_buffer.awaiting_video_resync {
                                        if let Some(reference) = ring_buffer.video_resync_reference {
                                            let gap = ClockTime::from_mseconds(33);
                                            let target = reference + gap;
                                            ring_buffer.video_pts_offset_ns =
                                                target.nseconds() as i64 - pts.nseconds() as i64;
                                            log::info!(
                                                "video resynced, raw_pts={}ms -> {}ms (offset {}ns)",
                                                pts.mseconds(), target.mseconds(), ring_buffer.video_pts_offset_ns
                                            );
                                        }
                                        ring_buffer.awaiting_video_resync = false;
                                    }

                                    let shifted = pts.nseconds() as i64 + ring_buffer.video_pts_offset_ns;
                                    pts = ClockTime::from_nseconds(shifted.max(0) as u64);
                                }
                                ContentType::Audio => {
                                    let now_ms = SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis() as u64;
                                    last_audio_frame_time.store(now_ms, Ordering::Release);

                                    if ring_buffer.awaiting_audio_resync {
                                        if let Some(reference) = ring_buffer.audio_resync_reference {
                                            let gap = ClockTime::from_mseconds(20);
                                            let target = reference + gap;
                                            ring_buffer.audio_pts_offset_ns =
                                                target.nseconds() as i64 - pts.nseconds() as i64;
                                        }
                                        ring_buffer.awaiting_audio_resync = false;
                                    }

                                    let shifted = pts.nseconds() as i64 + ring_buffer.audio_pts_offset_ns;
                                    pts = ClockTime::from_nseconds(shifted.max(0) as u64);
                                }
                            }

                            // Basically, for SOME reason, the video PTS starts off waaay late,
                            // which de-syncs the audio frame PTS's, meaning we gotta record first
                            // value, to then subtract it from all the others, but ONLY inside the
                            // calculations, cant modify original pts value
                            if ring_buffer.video_first_pts.is_none()
                                && let ContentType::Video = content_type
                            {
                                ring_buffer.video_first_pts = Some(pts);
                                ring_buffer.video_start_instant = Some(Instant::now());
                                log::debug!("First Video PTS Recieved: {}ms", pts.mseconds());
                            }

                            if ring_buffer.audio_first_pts.is_none()
                                && let ContentType::Audio = content_type
                            {
                                ring_buffer.audio_first_pts = Some(pts);
                                ring_buffer.audio_start_instant = Some(Instant::now());
                                log::debug!("First Audio PTS Recieved: {}ms", pts.mseconds());
                            }

                            #[cfg(debug_assertions)]
                            {
                                let limiter = match content_type {
                                    ContentType::Video => &mut ring_buffer.arriving_video_limiter,
                                    ContentType::Audio => &mut ring_buffer.arriving_audio_limiter,
                                };

                                if limiter.allow() {
                                    log::trace!(
                                        "{} PTS Arriving: {}ms ({} log/s)",
                                        content_type,
                                        pts.mseconds(),
                                        limiter.interval.as_secs()
                                    );
                                }
                            }

                            let dts = buffer_ref.dts();
                            let duration = buffer_ref.duration();
                            let is_keyframe = match content_type {
                                ContentType::Video => !buffer_ref
                                    .flags()
                                    .contains(gstreamer::BufferFlags::DELTA_UNIT),
                                ContentType::Audio => true,
                            };

                            //let data = buffer_ref
                            //    .map_readable()
                            //    .map_err(|_| gstreamer::FlowError::Error)?
                            //    .as_slice()
                            //    .to_vec();

                            let frame = EncodedFrame::new(buffer_ref.to_owned(), pts, dts, duration, is_keyframe);

                            if let Some(caps) = caps {
                                match content_type {
                                    ContentType::Video => {
                                        if ring_buffer.video_caps.is_none() {
                                            log::debug!("Stored downstream video caps: {:?}", caps);
                                            ring_buffer.video_caps = Some(caps);
                                        }
                                    }
                                    ContentType::Audio => {
                                        if ring_buffer.audio_caps.is_none() {
                                            log::debug!("Stored downstream audio caps: {}", caps);
                                            ring_buffer.audio_caps = Some(caps)
                                        }
                                    }
                                }
                            }

                            let res = match content_type {
                                ContentType::Video => {
                                    ring_buffer.push_video_frame(frame)
                                }
                                ContentType::Audio => {
                                    ring_buffer.push_audio_frame(frame)
                                }
                            };
                            match res {
                                Ok(_) => (),
                                Err(e) => {
                                     error!("Pushing {} frames error: {}", content_type, e);
                                     return Err(gstreamer::FlowError::Error);
                                }
                            }
                        }
                        Err(e) => {
                            error!("Ring buffer lock poisoned ({}): {}", content_type, e);
                            return Err(gstreamer::FlowError::Error);
                        }
                    }
                    Ok(gstreamer::FlowSuccess::Ok)
                })
                .build(),
        );
        Ok(())
    }

    pub fn build_mix_pipeline(
        &self,
        pipeline: &gstreamer::Pipeline,
        audio_settings: &AudioSettings,
    ) -> Result<gstreamer::Element, WayclipError> {
        let mix = gstreamer::ElementFactory::make("audiomixer")
            .name("audio_mixer")
            .property_from_str("start-time-selection", "zero")
            .property("ignore-inactive-pads", true)
            .build()?;

        let audioconvert = gstreamer::ElementFactory::make("audioconvert")
            .name("audio_mix_convert")
            .build()?;

        let caps_filter_audioconvert = gstreamer::ElementFactory::make("capsfilter")
            .name("audio_mix_caps_filter")
            .build()?;

        let caps_audioconvert = gstreamer::Caps::builder("audio/x-raw")
            .field("channels", DEFAULT_AUDIO_CHANNELS)
            .build();

        caps_filter_audioconvert.set_property("caps", &caps_audioconvert);

        let encoder = gstreamer::ElementFactory::make(audio_settings.codec.get_encoder())
            .name("audio_mixer_encoder")
            .build()?;

        let parser = gstreamer::ElementFactory::make(audio_settings.codec.get_parser())
            .name("audio_mixer_parser")
            .build()?;

        let audio_queue = gstreamer::ElementFactory::make("queue")
            .name("audio_mixer_post_queue")
            .build()?;

        let mix_pipeline = [
            mix.clone(),
            audioconvert,
            caps_filter_audioconvert,
            encoder,
            parser,
            audio_queue.clone(),
        ];

        for element in mix_pipeline.iter() {
            pipeline.add(element)?
        }

        gstreamer::Element::link_many(
            mix_pipeline
                .iter()
                .collect::<Vec<&gstreamer::Element>>()
                .as_slice(),
        )?;

        let audio_appsink = gstreamer_app::AppSink::builder()
            .name("audio_appsink")
            .property("drop", DEFAULT_APPSINK_DROP)
            .property("max-buffers", DEFAULT_APPSINK_MAX_BUFFERS)
            .property("sync", DEFAULT_APPSINK_SYNC)
            .build();

        pipeline.add(audio_appsink.upcast_ref::<gstreamer::Element>())?;
        audio_queue.link(audio_appsink.upcast_ref::<gstreamer::Element>())?;

        self.set_appsink_callbacks(&audio_appsink, ContentType::Audio)?;

        Ok(mix)
    }
}
