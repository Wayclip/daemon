use crate::linux::core::DaemonCore;
use gilrs::{EventType, Gilrs};
use log::{debug, error};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use wayclip_core::models::{error::WayclipError, input::controller::WayclipControllerCombo};

pub struct ControllerManager {
    pub daemon: Arc<Mutex<DaemonCore>>,
    pub trigger_combo: WayclipControllerCombo,
}

impl ControllerManager {
    pub fn new(daemon: Arc<Mutex<DaemonCore>>, trigger_combo: WayclipControllerCombo) -> Self {
        Self {
            daemon,
            trigger_combo,
        }
    }

    pub fn setup(&self, cancel_token: CancellationToken) -> Result<(), WayclipError> {
        let mut gilrs = Gilrs::new()?;
        let daemon_handle = self.daemon.clone();
        let combo = self.trigger_combo.clone();

        tokio::task::spawn_blocking(move || {
            let mut held: HashSet<gilrs::Button> = HashSet::new();
            let mut combo_already_triggered = false;

            while !cancel_token.is_cancelled() {
                if let Some(event) = gilrs.next_event_blocking(Some(Duration::from_millis(100))) {
                    match event.event {
                        EventType::ButtonPressed(button, _) => {
                            held.insert(button);

                            if combo.is_satisfied(&held) && !combo_already_triggered {
                                debug!("Controller combo triggered");
                                combo_already_triggered = true;

                                let d_clone = daemon_handle.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = DaemonCore::save_clip(d_clone, None).await {
                                        error!("Error saving clip via controller: {e}");
                                    }
                                });
                            }
                        }
                        EventType::ButtonReleased(button, _) => {
                            held.remove(&button);

                            if !combo.is_satisfied(&held) {
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
}
