use gstreamer::{
    ClockTime, Element, MessageType, MessageView, State, StateChangeSuccess,
    glib::object::{Cast, IsA},
    prelude::{ElementExt, ElementExtManual, GstBinExt, GstObjectExt},
};
use gstreamer_gl::{GL_DISPLAY_CONTEXT_TYPE, prelude::ContextGLExt};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use wayclip_core::models::error::WayclipError;

use crate::common::gst::bus::{BusWatcher, CoreEvent};

#[derive(Clone, Debug)]
pub struct GStreamerPipeline {
    pipeline: gstreamer::Pipeline,
    // We have to reserve back to Arc<Mutex<>> to track all the elements added so we can link stuff
    // together easily
    tracked_elements: Arc<Mutex<Vec<Element>>>,
}

pub struct GetStateResult {
    pub current: State,
    pub pending: State,
    pub success: StateChangeSuccess,
}

impl GStreamerPipeline {
    pub fn new() -> Self {
        let _ = gstreamer::init();
        Self {
            pipeline: gstreamer::Pipeline::new(),
            tracked_elements: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn spawn_watcher(
        &self,
        sender: mpsc::Sender<CoreEvent>,
    ) -> Result<BusWatcher, WayclipError> {
        Ok(BusWatcher::spawn(
            self.pipeline
                .bus()
                .ok_or_else(|| WayclipError::NotFound("No bus found".into()))?,
            sender,
        ))
    }

    pub fn raw(&self) -> &gstreamer::Pipeline {
        &self.pipeline
    }

    pub fn _get_state(&self, timeout: Option<ClockTime>) -> Result<GetStateResult, WayclipError> {
        let (res, current, pending) = self.pipeline.state(timeout);
        let success = res?;
        Ok(GetStateResult {
            current,
            pending,
            success,
        })
    }

    pub fn set_state(&self, state: gstreamer::State) -> Result<(), WayclipError> {
        self.pipeline.set_state(state)?;
        Ok(())
    }

    pub fn set_initial_time(&self, time: ClockTime) -> Result<(), WayclipError> {
        self.pipeline.set_start_time(time);
        self.pipeline.set_base_time(time);
        Ok(())
    }

    pub fn add(&self, element: &impl IsA<Element>) -> Result<(), WayclipError> {
        let element = element.upcast_ref::<Element>();
        self.pipeline.add(element)?;
        self.tracked_elements.lock().unwrap().push(element.clone());
        Ok(())
    }

    pub fn add_many<'a, T: IsA<Element> + 'a>(
        &self,
        elements: impl IntoIterator<Item = &'a T>,
    ) -> Result<(), WayclipError> {
        let mut tracked = self.tracked_elements.lock().unwrap();
        for element in elements {
            let elem = element.upcast_ref::<Element>();
            self.pipeline.add(elem)?;
            tracked.push(elem.clone());
        }
        Ok(())
    }

    pub fn link(
        &self,
        src: &impl IsA<Element>,
        dest: &impl IsA<Element>,
    ) -> Result<(), WayclipError> {
        let src_ref = src.upcast_ref::<Element>();
        let dest_ref = dest.upcast_ref::<Element>();

        src_ref.link(dest_ref).map_err(|_| {
            WayclipError::Remux(
                format!(
                    "Failed to link element '{}' to '{}'",
                    src_ref.name(),
                    dest_ref.name()
                )
                .into(),
            )
        })
    }

    pub fn add_and_link(&self, elements: &[&impl IsA<Element>]) -> Result<(), WayclipError> {
        if elements.is_empty() {
            return Ok(());
        }
        for elem in elements {
            self.add(*elem)?;
        }

        for window in elements.windows(2) {
            self.link(window[0], window[1])?;
        }

        Ok(())
    }

    pub fn link_all(&self) -> Result<(), WayclipError> {
        let tracked = self.tracked_elements.lock().unwrap();
        for window in tracked.windows(2) {
            self.link(&window[0], &window[1])?;
        }
        Ok(())
    }

    pub fn last_element(&self) -> Option<Element> {
        self.tracked_elements.lock().unwrap().last().cloned()
    }

    pub fn wait_eos(self, timeout: ClockTime) -> Result<(), WayclipError> {
        let bus = self
            .pipeline
            .bus()
            .ok_or_else(|| WayclipError::Remux("No bus found".into()))?;

        let deadline =
            std::time::Instant::now() + std::time::Duration::from_nanos(timeout.nseconds());
        let result = loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let wait = ClockTime::from_nseconds(left.as_nanos() as u64);
            match bus.timed_pop_filtered(
                wait,
                &[MessageType::Eos, MessageType::Error, MessageType::Warning],
            ) {
                Some(message) => match message.view() {
                    MessageView::Eos(_) => {
                        log::debug!("Pipeline completed, EOS");
                        break Ok(());
                    }
                    MessageView::Error(e) => {
                        log::error!("Pipeline error: {:?}", e);
                        break Err(WayclipError::Remux(
                            format!("Pipeline error: {:?}", e).into(),
                        ));
                    }
                    MessageView::Warning(w) => {
                        log::warn!("Pipeline warning: {:?}", w);
                    }
                    _ => {}
                },
                None => {
                    break Err(WayclipError::Remux(
                        format!("Pipeline timed out after {}s", timeout).into(),
                    ));
                }
            }
        };

        self.pipeline.set_state(gstreamer::State::Null)?;
        result
    }

    pub fn last_error_message(&self, timeout: ClockTime) -> Option<String> {
        let bus = self.pipeline.bus()?;
        while let Some(msg) = bus.timed_pop(timeout) {
            if let MessageView::Error(err) = msg.view() {
                let src_name = err.src().map(|s| s.to_string()).unwrap_or_default();
                return Some(format!(
                    "{} ({:?}) from element {}",
                    err.error(),
                    err.debug(),
                    src_name
                ));
            }
        }
        None
    }

    pub fn play_and_wait_ready(&self, timeout: ClockTime) -> Result<(), WayclipError> {
        if let Err(e) = self.pipeline.set_state(State::Playing) {
            let reason = self
                .last_error_message(ClockTime::from_mseconds(500))
                .unwrap_or_else(|| "unknown".to_string());
            let _ = self.pipeline.set_state(State::Null);
            return Err(WayclipError::Validation(
                format!("Failed to set Playing synchronously ({e:?}): {reason}").into(),
            ));
        }

        let (state_result, current_state, _) = self.pipeline.state(Some(timeout));
        if state_result.is_err() || current_state != State::Playing {
            let reason = self
                .last_error_message(ClockTime::ZERO)
                .unwrap_or_else(|| "unknown".to_string());
            let _ = self.pipeline.set_state(State::Null);
            return Err(WayclipError::Validation(
                format!("Pipeline failed to reach PLAYING state ({state_result:?}): {reason}")
                    .into(),
            ));
        }

        Ok(())
    }

    pub fn play_and_wait_eos(
        &self,
        state_timeout: ClockTime,
        eos_timeout: ClockTime,
    ) -> Result<(), WayclipError> {
        self.pipeline.set_state(gstreamer::State::Playing)?;

        if self.pipeline.state(Some(state_timeout)).0.is_err() {
            let bus = self
                .pipeline
                .bus()
                .ok_or_else(|| WayclipError::Remux("No pipeline bus".into()))?;
            let mut reason = "unknown".to_string();
            while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::ZERO) {
                if let gstreamer::MessageView::Error(e) = msg.view() {
                    reason = format!("{} ({:?})", e.error(), e.debug());
                    break;
                }
            }
            let _ = self.pipeline.set_state(gstreamer::State::Null);
            return Err(WayclipError::Remux(
                format!("Pipeline failed to reach PLAYING state for preview: {reason}").into(),
            ));
        }

        self.clone().wait_eos(eos_timeout)?;

        Ok(())
    }

    pub fn bind_gl_display(
        &mut self,
        gl_display: &gstreamer_gl::GLDisplay,
    ) -> Result<(), WayclipError> {
        let mut context = gstreamer::Context::new(GL_DISPLAY_CONTEXT_TYPE.as_str(), true);
        context
            .get_mut()
            .ok_or_else(|| {
                WayclipError::NotFound("Newly constructed context has unique ref".into())
            })?
            .set_gl_display(gl_display);

        self.pipeline.set_context(&context);

        let bus = self
            .pipeline
            .bus()
            .ok_or_else(|| WayclipError::NotFound("Pipeline has no message bus".into()))?;

        bus.set_sync_handler(move |_, msg| {
            let gstreamer::MessageView::NeedContext(need_ctx) = msg.view() else {
                return gstreamer::BusSyncReply::Pass;
            };

            if need_ctx.context_type() != *gstreamer_gl::GL_DISPLAY_CONTEXT_TYPE {
                return gstreamer::BusSyncReply::Pass;
            }
            if let Some(src) = msg
                .src()
                .and_then(|s| s.downcast_ref::<gstreamer::Element>())
            {
                src.set_context(&context);
            }

            gstreamer::BusSyncReply::Pass
        });

        Ok(())
    }
}
