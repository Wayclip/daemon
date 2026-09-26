use gstreamer::{
    ClockTime, Element, MessageView, State, StateChangeSuccess,
    glib::object::{Cast, IsA},
    prelude::{ElementExt, GstBinExt},
};
use wayclip_core::models::error::WayclipError;

#[derive(Clone, Debug)]
pub struct GStreamerPipeline {
    pipeline: gstreamer::Pipeline,
}

pub struct GetStateResult {
    current: State,
    pending: State,
    success: StateChangeSuccess,
}

impl GStreamerPipeline {
    pub fn new() -> Self {
        let _ = gstreamer::init();
        Self {
            pipeline: gstreamer::Pipeline::new(),
        }
    }

    pub fn get_state(&self, timeout: Option<ClockTime>) -> Result<GetStateResult, WayclipError> {
        let (res, current, pending) = self.pipeline.state(timeout);
        let success = res?;
        Ok(GetStateResult {
            current,
            pending,
            success,
        })
    }

    pub fn set_state(self, state: gstreamer::State) -> Result<Self, WayclipError> {
        self.pipeline.set_state(state)?;
        Ok(self)
    }

    pub fn add(self, element: &impl IsA<Element>) -> Result<Self, WayclipError> {
        self.pipeline.add(element.upcast_ref())?;
        Ok(self)
    }

    pub fn add_many<'a, T: IsA<Element> + 'a>(
        self,
        elements: impl IntoIterator<Item = &'a T>,
    ) -> Result<Self, WayclipError> {
        for element in elements {
            self.pipeline.add(element.upcast_ref())?;
        }
        Ok(self)
    }

    pub fn wait_eos(self, timeout: ClockTime) -> Result<(), WayclipError> {
        let bus = self
            .pipeline
            .bus()
            .ok_or_else(|| WayclipError::Remux("No bus found".into()))?;

        let result = loop {
            match bus.timed_pop(timeout) {
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
}
