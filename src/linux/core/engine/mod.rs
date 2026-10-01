use ::gstreamer::ClockTime;
use parking_lot::Mutex;
use std::sync::Arc;
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

use crate::{
    common::video::ring::RingBuffer,
    linux::core::{
        engine::{
            gstreamer::{DaemonEngineGStreamer, save::SavePipelineFactory},
            pipewire::DaemonEnginePipewire,
        },
        session::CurrentSession,
    },
};

pub mod gstreamer;
pub mod pipewire;

/// The DaemonEngine struct will be responsible for handing the main process of recording the
/// screen, in addition to storing all the required fields and pipelines
pub struct DaemonEngine {
    pipewire_pipeline: DaemonEnginePipewire,
    gstreamer_pipeline: DaemonEngineGStreamer,
    /// The only mutex will be only on the ring_buffer, which is what we actually need to keep safe
    ring_buffer: Arc<Mutex<RingBuffer>>,
}

impl DaemonEngine {
    pub fn new(max_duration: ClockTime) -> Result<Self, WayclipError> {
        Ok(Self {
            gstreamer_pipeline: DaemonEngineGStreamer::new()?,
            pipewire_pipeline: DaemonEnginePipewire::new()?,
            ring_buffer: Arc::new(Mutex::new(RingBuffer::new(max_duration))),
        })
    }

    pub async fn stop(&mut self) -> Result<(), WayclipError> {
        let gst = self.gstreamer_pipeline.stop();
        let pw = self.pipewire_pipeline.stop().await;
        gst.and(pw)
    }

    pub async fn setup(&mut self, user_settings: &UserSettings) -> Result<(), WayclipError> {
        self.pipewire_pipeline.setup_screncast().await?;

        self.gstreamer_pipeline.setup_gstreamer(
            user_settings,
            &self.pipewire_pipeline.connection_data,
            &self.pipewire_pipeline.manager,
            Arc::clone(&self.ring_buffer),
        )?;

        self.gstreamer_pipeline.start().await?;

        Ok(())
    }

    pub async fn save(
        &mut self,
        current_session: &CurrentSession,
        forced_name: Option<String>,
    ) -> Result<(), WayclipError> {
        SavePipelineFactory::save(current_session, forced_name, Arc::clone(&self.ring_buffer)).await
    }
}
