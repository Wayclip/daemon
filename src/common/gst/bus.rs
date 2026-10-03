use gstreamer::{ClockTime, MessageView, prelude::GstObjectExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub enum CoreEvent {
    PipelineError(String),
    PipelineEos,
}

pub struct BusWatcher {
    token: CancellationToken,
}

impl BusWatcher {
    pub fn spawn(bus: gstreamer::Bus, events: mpsc::Sender<CoreEvent>) -> Self {
        let token = CancellationToken::new();
        let loop_token = token.clone();

        tokio::task::spawn_blocking(move || {
            while !loop_token.is_cancelled() {
                let Some(msg) = bus.timed_pop(ClockTime::from_mseconds(500)) else {
                    continue;
                };

                let event = match msg.view() {
                    MessageView::Error(e) => {
                        let src = e
                            .src()
                            .map(|s| s.path_string().to_string())
                            .unwrap_or_default();
                        Some(CoreEvent::PipelineError(format!(
                            "{} ({:?}) from {src}",
                            e.error(),
                            e.debug()
                        )))
                    }
                    MessageView::Eos(_) => Some(CoreEvent::PipelineEos),
                    MessageView::Warning(w) => {
                        log::warn!("pipeline warning: {w:?}");
                        None
                    }
                    _ => None,
                };

                if let Some(ev) = event {
                    let _ = events.blocking_send(ev);
                    break;
                }
            }
        });

        Self { token }
    }

    pub fn stop(&self) {
        self.token.cancel();
    }
}

impl Drop for BusWatcher {
    fn drop(&mut self) {
        self.token.cancel();
    }
}
