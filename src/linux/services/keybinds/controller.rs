use crate::linux::ipc::commands::IpcCommand;
use gilrs::{EventType, Gilrs};
use log::debug;
use std::{collections::HashSet, time::Duration};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use wayclip_core::models::{error::WayclipError, input::controller::WayclipControllerCombo};

pub struct ControllerManager {
    cancel_token: Option<CancellationToken>,
}

impl ControllerManager {
    pub fn new() -> Self {
        Self { cancel_token: None }
    }

    pub fn start(
        &mut self,
        trigger_combo: WayclipControllerCombo,
        command_sender: mpsc::Sender<IpcCommand>,
    ) -> Result<(), WayclipError> {
        self.stop();
        let mut gilrs = Gilrs::new()?;
        let token = CancellationToken::new();
        let loop_token = token.clone();
        self.cancel_token = Some(token);

        tokio::task::spawn_blocking(move || {
            let mut held: HashSet<gilrs::Button> = HashSet::new();
            let mut combo_already_triggered = false;

            while !loop_token.is_cancelled() {
                if let Some(event) = gilrs.next_event_blocking(Some(Duration::from_millis(100))) {
                    match event.event {
                        EventType::ButtonPressed(button, _) => {
                            held.insert(button);

                            if trigger_combo.is_satisfied(&held) && !combo_already_triggered {
                                debug!("Controller combo triggered");
                                combo_already_triggered = true;

                                let tx = command_sender.clone();
                                tokio::spawn(async move {
                                    let (sender, receiver) = oneshot::channel();
                                    if tx
                                        .send(IpcCommand::SaveClip {
                                            custom_name: None,
                                            responder: sender,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        log::warn!("Daemon core offline, could not trigger clip");
                                        return;
                                    }

                                    match receiver.await {
                                        Ok(Ok(clip_name)) => {
                                            log::info!(
                                                "Clip saved successfully via controller: {clip_name}"
                                            );
                                        }
                                        Ok(Err(e)) => log::error!("Failed to save clip: {e:?}"),
                                        Err(_) => log::warn!("Daemon core dropped reply channel"),
                                    }
                                });
                            }
                        }
                        EventType::ButtonReleased(button, _) => {
                            held.remove(&button);

                            if !trigger_combo.is_satisfied(&held) {
                                combo_already_triggered = false;
                            }
                        }
                        EventType::Disconnected => {
                            held.clear();
                            combo_already_triggered = false;
                        }
                        _ => {}
                    }
                }
            }
        });

        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(t) = self.cancel_token.take() {
            t.cancel();
        }
    }
}

impl Drop for ControllerManager {
    fn drop(&mut self) {
        self.stop();
    }
}
