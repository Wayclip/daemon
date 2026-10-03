use ::gstreamer::ClockTime;
use parking_lot::Mutex;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

use crate::{
    common::{
        gst::bus::{BusWatcher, CoreEvent},
        video::ring::RingBuffer,
    },
    linux::{
        engine::{
            gstreamer::{DaemonEngineGStreamer, save::SavePipelineFactory},
            pipewire::DaemonEnginePipewire,
        },
        session::CurrentSession,
    },
};

pub const STALL_LIMIT: Duration = Duration::from_secs(10);

pub mod gstreamer;
pub mod pipewire;

/// The DaemonEngine struct will be responsible for handing the main process of recording the
/// screen, in addition to storing all the required fields and pipelines
pub struct DaemonEngine {
    pipewire_pipeline: DaemonEnginePipewire,
    gstreamer_pipeline: DaemonEngineGStreamer,
    /// The only mutex will be only on the ring_buffer, which is what we actually need to keep safe
    ring_buffer: Arc<Mutex<RingBuffer>>,
    pub watcher: Option<BusWatcher>,
    started_at: Option<Instant>,
}

impl DaemonEngine {
    /// Create a new DaemonEngine with a maximum duration for the ring buffer
    pub fn new(max_duration: ClockTime) -> Result<Self, WayclipError> {
        Ok(Self {
            gstreamer_pipeline: DaemonEngineGStreamer::new()?,
            pipewire_pipeline: DaemonEnginePipewire::new()?,
            ring_buffer: Arc::new(Mutex::new(RingBuffer::new(max_duration))),
            watcher: None,
            started_at: None,
        })
    }

    /// Method to safely stop the pipeline, managers and watcher.
    pub async fn stop(&mut self) -> Result<(), WayclipError> {
        let gst = self.gstreamer_pipeline.stop();
        let pw = self.pipewire_pipeline.stop().await;
        if let Some(ref wa) = self.watcher {
            wa.stop();
        }
        gst.and(pw)
    }

    /// Returns the clone of the ring
    pub fn ring(&self) -> Arc<Mutex<RingBuffer>> {
        Arc::clone(&self.ring_buffer)
    }

    /// Method to setup and start the engine, and start recording straight away
    pub async fn setup(
        &mut self,
        user_settings: &UserSettings,
        sender: mpsc::Sender<CoreEvent>,
    ) -> Result<(), WayclipError> {
        self.pipewire_pipeline.setup_screncast().await?;

        self.gstreamer_pipeline.setup_gstreamer(
            user_settings,
            &self.pipewire_pipeline.connection_data,
            &self.pipewire_pipeline.manager,
            Arc::clone(&self.ring_buffer),
        )?;

        self.gstreamer_pipeline.start().await?;

        self.watch(sender)?;
        self.started_at = Some(Instant::now());

        Ok(())
    }

    pub fn is_stalled(&self, limit: Duration) -> bool {
        let Some(started) = self.started_at else {
            return false;
        };

        match self.ring_buffer.lock().video_last_instant {
            Some(last) => last.elapsed() > limit,
            None => started.elapsed() > limit,
        }
    }

    pub async fn recover(
        &mut self,
        user_settings: &UserSettings,
        sender: mpsc::Sender<CoreEvent>,
    ) -> Result<(), WayclipError> {
        self.started_at = None;

        let old = self.gstreamer_pipeline.pipeline.clone();
        match tokio::task::spawn_blocking(move || old.set_state(::gstreamer::State::Null)).await {
            Ok(Err(e)) => log::warn!("old pipeline NULL failed: {e}"),
            Err(e) => log::warn!("NULL task failed: {e}"),
            Ok(Ok(())) => {}
        }

        if let Err(e) = self.pipewire_pipeline.stop().await {
            log::warn!("closing old portal session error: {e}");
        }

        {
            let mut ring = self.ring_buffer.lock();
            ring.begin_resync();
            ring.video_last_instant = None;
            ring.audio_last_instant = None;
        }

        self.gstreamer_pipeline = DaemonEngineGStreamer::new()?;
        self.setup(user_settings, sender).await
    }

    fn watch(&mut self, sender: mpsc::Sender<CoreEvent>) -> Result<(), WayclipError> {
        self.watcher = Some(self.gstreamer_pipeline.pipeline.spawn_watcher(sender)?);
        Ok(())
    }

    pub fn stop_watcher(&mut self) {
        if let Some(ref w) = self.watcher {
            w.stop();
        }
        self.watcher = None;
    }
}
