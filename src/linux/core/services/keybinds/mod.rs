use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

use crate::linux::core::{
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
    pub fn new(
        user_settings: &UserSettings,
        cancel_token: CancellationToken,
    ) -> Result<Self, WayclipError> {
        Ok(Self {
            desktop: DesktopManager::new(user_settings.shortcuts.save_clip.clone())?,
            controller: ControllerManager::new(cancel_token),
        })
    }

    pub fn setup_keybinds(
        &mut self,
        user_settings: &UserSettings,
        command_sender: &mpsc::Sender<IpcCommand>,
    ) -> Result<(), WayclipError> {
        if let Some(combo) = user_settings.shortcuts.save_clip_controller.clone() {
            self.controller.start(combo, command_sender.clone())?;
        }
        self.desktop.create_auto_bind(command_sender)?;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), WayclipError> {
        self.controller.stop();
        self.desktop.remove_auto_bind()
    }
}
