use tokio::sync::mpsc;
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

use crate::linux::{
    ipc::commands::IpcCommand,
    services::keybinds::{controller::ControllerManager, desktop::DesktopManager},
};

pub mod controller;
pub mod desktop;

pub struct KeybindsService {
    pub desktop: DesktopManager,
    pub controller: ControllerManager,
}

impl KeybindsService {
    pub fn new(user_settings: &UserSettings) -> Result<Self, WayclipError> {
        Ok(Self {
            desktop: DesktopManager::new(user_settings.shortcuts.save_clip.clone())?,
            controller: ControllerManager::new(),
        })
    }

    pub fn setup_keybinds(
        &mut self,
        user_settings: &UserSettings,
        command_sender: &mpsc::Sender<IpcCommand>,
    ) -> Result<(), WayclipError> {
        if let Some(combo) = user_settings.shortcuts.save_clip_controller.clone() {
            if let Err(e) = self.controller.start(combo, command_sender.clone()) {
                log::error!("Controller keybind failed: {e}");
            }
        }

        let sender = command_sender.clone();
        let trigger = user_settings.shortcuts.save_clip.clone();

        tokio::spawn(async move {
            let mut backoff = std::time::Duration::from_secs(2);

            loop {
                let mut desktop = match DesktopManager::new(trigger.clone()) {
                    Ok(d) => d,
                    Err(e) => {
                        log::error!("Desktop keybind setup failed: {e}");
                        return;
                    }
                };

                if desktop.is_ready() {
                    match desktop.create_auto_bind(&sender) {
                        Ok(()) => {
                            log::info!("Desktop keybind registered");
                            return;
                        }
                        Err(e) => {
                            log::error!("Desktop keybind failed, retrying: {e}");
                        }
                    }
                }

                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
            }
        });

        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), WayclipError> {
        self.controller.stop();
        self.desktop.remove_auto_bind()
    }
}
