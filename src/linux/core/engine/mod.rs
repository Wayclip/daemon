use ::gstreamer::ClockTime;
use std::sync::Arc;
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

use crate::{
    common::video::ring::RingBuffer,
    linux::core::engine::{gstreamer::DaemonEngineGStreamer, pipewire::DaemonEnginePipewire},
};

pub mod gstreamer;
pub mod pipewire;

/// The DaemonEngine struct will be responsible for handing the main process of recording the
/// screen, in addition to storing all the required fields and pipelines
pub struct DaemonEngine {
    pipewire_pipeline: DaemonEnginePipewire,
    gstreamer_pipeline: DaemonEngineGStreamer,
    /// The only mutex will be only on the ring_buffer, which is what we actually need to keep safe
    ring_buffer: Arc<parking_lot::Mutex<RingBuffer>>,
}

impl DaemonEngine {
    pub fn new(max_duration: ClockTime) -> Result<Self, WayclipError> {
        Ok(Self {
            gstreamer_pipeline: DaemonEngineGStreamer::new()?,
            pipewire_pipeline: DaemonEnginePipewire::new()?,
            ring_buffer: Arc::new(parking_lot::Mutex::new(RingBuffer::new(max_duration))),
        })
    }

    pub async fn stop(&mut self) -> Result<(), WayclipError> {
        let gst = self.gstreamer_pipeline.stop();
        let pw = self.pipewire_pipeline.stop().await;
        gst.and(pw)
    }

    pub async fn setup(&mut self, user_settings: &UserSettings) -> Result<(), WayclipError> {
        self.pipewire_pipeline.setup_screncast().await?;

        // will need to pass in MIXER element and THEN generate the pads.
        self.pipewire_pipeline.audio_setup(
            &self.gstreamer_pipeline.pipeline,
            user_settings,
            sink_pad,
        )
    }
}
