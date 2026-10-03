use gstreamer::ClockTime;
use gstreamer::Element;
use gstreamer::{State, glib::object::Cast};
use gstreamer_app::AppSinkCallbacks;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Instant;
use wayclip_core::models::error::WayclipError;
use wayclip_core::settings::UserSettings;

use crate::common::gst::app::GStreamerApp;
use crate::common::video::ring::{
    RingBuffer,
    data::{ContentType, EncodedFrame},
};
use crate::linux::engine::gstreamer::audio::AudioBranchBuilder;
use crate::linux::engine::gstreamer::audio::device::AudioDeviceFactory;
use crate::linux::engine::gstreamer::video::VideoBranchBuilder;
use crate::linux::engine::pipewire::DaemonEngineConnectionData;
use crate::linux::engine::pipewire::manager::PipewireManager;
use wayclip_core::settings::recording::CodecType;

const DEFAULT_CONFIG_INTERVAL: i32 = 1;
const DEFAULT_GST_LEAKY_DOWNSTREAM: &str = "2";
const DEFAULT_MAX_SIZE_BUFFER: u32 = 2;
const DEFAULT_MAX_SIZE_BYTES: u32 = 0;
const DEFAULT_MAX_SIZE_TIME_NS: u64 = 0;
const DEFAULT_GOP_SIZE: i32 = 30;
const DEFAULT_KEYFRAME_PERIOD: u32 = 30;

pub mod audio;
pub mod save;
pub mod video;

use crate::common::gst::pipeline::GStreamerPipeline;

pub type ParserFilter = (gstreamer::Element, Option<gstreamer::Element>);

// GStreamer related actions
pub struct DaemonEngineGStreamer {
    pub pipeline: GStreamerPipeline,
}

impl DaemonEngineGStreamer {
    pub fn new() -> Result<Self, WayclipError> {
        // The GStreamerPipeline will already initialise gstreamer::init(), so we can safely call to
        // get the GLDisplayEGL
        let pipeline = GStreamerPipeline::new();
        Ok(Self { pipeline: pipeline })
    }

    pub fn stop(&mut self) -> Result<(), WayclipError> {
        // Setting state to Null already tears down the whole pipeline
        self.pipeline.set_state(State::Null)?;
        Ok(())
    }

    pub fn setup_gstreamer(
        &mut self,
        user_settings: &UserSettings,
        connection_data: &DaemonEngineConnectionData,
        manager: &PipewireManager,
        ring: Arc<Mutex<RingBuffer>>,
    ) -> Result<(), WayclipError> {
        if matches!(
            user_settings.recording.video.codec.get_backend(),
            CodecType::NVIDIA
        ) {
            let gl_display =
                gstreamer_gl_egl::GLDisplayEGL::new()?.upcast::<gstreamer_gl::GLDisplay>();
            self.pipeline.bind_gl_display(&gl_display)?;
        }

        self.setup_video(user_settings, connection_data, Arc::clone(&ring))?;
        self.setup_audio(user_settings, manager, Arc::clone(&ring))?;

        Ok(())
    }

    pub async fn start(&mut self) -> Result<(), WayclipError> {
        self.pipeline.set_initial_time(ClockTime::ZERO)?;

        let pipeline_clone = self.pipeline.clone();

        tokio::task::spawn_blocking(move || {
            pipeline_clone.play_and_wait_ready(gstreamer::ClockTime::from_seconds(10))
        })
        .await
        .map_err(|e| WayclipError::Validation(e.to_string().into()))?
    }

    fn setup_video(
        &mut self,
        user_settings: &UserSettings,
        connection_data: &DaemonEngineConnectionData,
        ring: Arc<Mutex<RingBuffer>>,
    ) -> Result<(), WayclipError> {
        let video_branch = VideoBranchBuilder::new(user_settings, connection_data).build()?;

        self.pipeline
            .add_and_link(video_branch.iter().collect::<Vec<&Element>>().as_slice())?;

        let video_appsink = GStreamerApp::build_app_sink();
        let video_appsink_ref = video_appsink.upcast_ref::<Element>();

        let last = self
            .pipeline
            .last_element()
            .ok_or_else(|| WayclipError::Video("No last video element".into()))?;

        self.pipeline.add(video_appsink_ref)?;
        self.pipeline.link(&last, video_appsink_ref)?;

        video_appsink.set_callbacks(self.build_callback(ring, ContentType::Video));

        Ok(())
    }

    fn setup_audio(
        &mut self,
        user_settings: &UserSettings,
        manager: &PipewireManager,
        ring: Arc<Mutex<RingBuffer>>,
    ) -> Result<(), WayclipError> {
        let audio_branch = AudioBranchBuilder::new(user_settings).build()?;

        self.pipeline
            .add_and_link(audio_branch.1.iter().collect::<Vec<&Element>>().as_slice())?;

        let audio_appsink = GStreamerApp::build_app_sink();
        let audio_appsink_ref = audio_appsink.upcast_ref::<Element>();

        let last = self
            .pipeline
            .last_element()
            .ok_or_else(|| WayclipError::Audio("No last audio element".into()))?;

        self.pipeline.add(audio_appsink_ref)?;
        self.pipeline.link(&last, audio_appsink_ref)?;

        audio_appsink.set_callbacks(self.build_callback(ring, ContentType::Audio));

        AudioDeviceFactory::setup_devices(&self.pipeline, user_settings, &audio_branch.0, manager)?;

        Ok(())
    }

    fn build_callback(
        &self,
        ring: Arc<parking_lot::Mutex<RingBuffer>>,
        content_type: ContentType,
    ) -> AppSinkCallbacks {
        AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                let buffer_ref = sample.buffer().ok_or(gstreamer::FlowError::Error)?;

                let caps = sample.caps().map(|caps| caps.to_owned());
                let mut pts = buffer_ref.pts().ok_or(gstreamer::FlowError::Error)?;

                let mut ring = ring.lock();

                match content_type {
                    ContentType::Video => {
                        ring.video_last_instant = Some(Instant::now());
                        if ring.awaiting_video_resync {
                            if let Some(reference) = ring.video_resync_reference {
                                let gap = ClockTime::from_mseconds(33);
                                ring.video_pts_offset_ns =
                                    (reference + gap).nseconds() as i64 - pts.nseconds() as i64;
                            }
                            ring.awaiting_video_resync = false;
                        }

                        let shifted = pts.nseconds() as i64 + ring.video_pts_offset_ns;
                        pts = ClockTime::from_nseconds(shifted.max(0) as u64);
                    }
                    ContentType::Audio => {
                        ring.audio_last_instant = Some(Instant::now());

                        if ring.awaiting_audio_resync {
                            if let Some(reference) = ring.audio_resync_reference {
                                let gap = ClockTime::from_mseconds(20);
                                ring.audio_pts_offset_ns =
                                    (reference + gap).nseconds() as i64 - pts.nseconds() as i64;
                            }
                            ring.awaiting_audio_resync = false;
                        }

                        let shifted = pts.nseconds() as i64 + ring.audio_pts_offset_ns;
                        pts = ClockTime::from_nseconds(shifted.max(0) as u64);
                    }
                }

                if ring.video_first_pts.is_none()
                    && let ContentType::Video = content_type
                {
                    ring.video_first_pts = Some(pts);
                    ring.video_start_instant = Some(Instant::now());
                    log::debug!("First Video PTS Recieved: {}ms", pts.mseconds());
                }

                if ring.audio_first_pts.is_none()
                    && let ContentType::Audio = content_type
                {
                    ring.audio_first_pts = Some(pts);
                    ring.audio_start_instant = Some(Instant::now());
                    log::debug!("First Audio PTS Recieved: {}ms", pts.mseconds());
                }

                let dts = buffer_ref.dts();
                let duration = buffer_ref.duration();
                let is_keyframe = match content_type {
                    ContentType::Video => !buffer_ref
                        .flags()
                        .contains(gstreamer::BufferFlags::DELTA_UNIT),
                    ContentType::Audio => true,
                };

                let frame =
                    EncodedFrame::new(buffer_ref.to_owned(), pts, dts, duration, is_keyframe);

                if let Some(caps) = caps {
                    match content_type {
                        ContentType::Video => {
                            if ring.video_caps.is_none() {
                                log::debug!("Stored downstream video caps: {:?}", caps);
                                ring.video_caps = Some(caps);
                            }
                        }
                        ContentType::Audio => {
                            if ring.audio_caps.is_none() {
                                log::debug!("Stored downstream audio caps: {}", caps);
                                ring.audio_caps = Some(caps)
                            }
                        }
                    }
                }

                let res = match content_type {
                    ContentType::Video => ring.push_video_frame(frame),
                    ContentType::Audio => ring.push_audio_frame(frame),
                };

                match res {
                    Ok(_) => Ok(gstreamer::FlowSuccess::Ok),
                    Err(e) => {
                        log::error!("Pushing {} frames error: {}", content_type, e);
                        Err(gstreamer::FlowError::Error)
                    }
                }
            })
            .build()
    }
}
