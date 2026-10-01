use crate::linux::core::ipc::commands::IpcCommand;
use gilrs::{EventType, Gilrs};
use log::debug;
use std::{collections::HashSet, time::Duration};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use wayclip_core::models::{error::WayclipError, input::controller::WayclipControllerCombo};

pub struct ControllerManager {
    cancel_token: CancellationToken,
}

impl ControllerManager {
    pub fn new(cancel_token: CancellationToken) -> Self {
        Self { cancel_token }
    }

    pub fn start(
        &self,
        trigger_combo: WayclipControllerCombo,
        command_sender: mpsc::Sender<IpcCommand>,
    ) -> Result<Self, WayclipError> {
        let mut gilrs = Gilrs::new()?;
        let cancel_token = CancellationToken::new();
        let loop_token = cancel_token.clone();

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
                                        Ok(Ok(())) => {
                                            log::info!("Clip saved successfully via hotkey")
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

        Ok(Self { cancel_token })
    }

    pub fn stop(&self) {
        self.cancel_token.cancel();
    }
}

impl Drop for ControllerManager {
    fn drop(&mut self) {
        self.cancel_token.cancel();
    }
}
