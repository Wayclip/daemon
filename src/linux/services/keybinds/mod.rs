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
        if let Err(e) = self.desktop.create_auto_bind(command_sender) {
            log::error!("Desktop keybind failed (bind manually): {e}");
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), WayclipError> {
        self.controller.stop();
        self.desktop.remove_auto_bind()
    }
}
