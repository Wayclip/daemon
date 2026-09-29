use ashpd::desktop::{Session, screencast::Screencast};
use gstreamer::{ClockTime, State, glib::object::Cast};
use std::{os::fd::OwnedFd, sync::Arc};
use wayclip_core::models::error::WayclipError;

use crate::{
    PipewireManager,
    common::{gst::pipeline::GStreamerPipeline, video::ring::RingBuffer},
};

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
        self.gstreamer_pipeline.stop()?;
        self.pipewire_pipeline.stop().await?;

        Ok(())
    }
}

/// --- GStreamer ---

// GStreamer related actions
struct DaemonEngineGStreamer {
    pipeline: Option<GStreamerPipeline>,
    gl_display: gstreamer_gl::GLDisplay,
}

impl DaemonEngineGStreamer {
    pub fn new() -> Result<Self, WayclipError> {
        // The GStreamerPipeline will already initialise gstreamer::init(), so we can safely call to
        // get the GLDisplayEGL
        let pipeline = GStreamerPipeline::new();
        let gl_display = gstreamer_gl_egl::GLDisplayEGL::new()?.upcast::<gstreamer_gl::GLDisplay>();

        Ok(Self {
            pipeline: Some(pipeline),
            gl_display,
        })
    }

    pub fn stop(&mut self) -> Result<(), WayclipError> {
        if let Some(pipeline) = self.pipeline.as_ref() {
            pipeline.set_state(State::Null)?;
        }
        self.pipeline = None;

        Ok(())
    }
}

/// --- Pipewire ---

// We use ashpd to capute the screen, however, the input is provided by pipewire anyway
struct DaemonEnginePipewire {
    manager: PipewireManager,
    connection_data: DaemonEngineConnectionData,
}

#[derive(Default)]
struct DaemonEngineConnectionData {
    proxy: Option<Screencast>,
    session: Option<Session<Screencast>>,
    file_descriptor: Option<OwnedFd>,
    node_id: Option<String>,
    restore_token: Option<String>,
}

impl DaemonEnginePipewire {
    pub fn new() -> Result<Self, WayclipError> {
        Ok(Self {
            // We initialise the pipewire manager, so that we can have constant access to it
            // allowing us to pull info about devices and more
            manager: PipewireManager::new()?,
            // Rest of variables are None, since we are only creating the instance and have not yet
            // captured any information
            connection_data: DaemonEngineConnectionData::default(),
        })
    }

    pub async fn stop(&mut self) -> Result<(), WayclipError> {
        self.connection_data.node_id = None;
        self.connection_data.file_descriptor = None;
        self.connection_data.proxy = None;

        if let Some(session) = self.connection_data.session.take() {
            session.close().await?;
        }

        Ok(())
    }
}
