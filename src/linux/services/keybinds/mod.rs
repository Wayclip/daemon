use parking_lot::Mutex;
use std::sync::Arc;
use tokio::{runtime::Handle, sync::mpsc};
use tokio_util::sync::CancellationToken;
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

use crate::linux::{
    ipc::commands::IpcCommand,
    services::keybinds::{controller::ControllerManager, desktop::DesktopManager},
    session::env::SessionVariable,
};

pub mod controller;
pub mod desktop;

pub struct KeybindsService {
    active_desktop: Arc<Mutex<Option<DesktopManager>>>,
    controller: ControllerManager,
    cancel_token: CancellationToken,
}

impl KeybindsService {
    pub fn new(cancel_token: CancellationToken) -> Self {
        Self {
            active_desktop: Arc::new(Mutex::new(None)),
            controller: ControllerManager::new(),
            cancel_token: cancel_token.child_token(),
        }
    }

    pub fn setup_keybinds(
        &mut self,
        user_settings: &UserSettings,
        command_sender: &mpsc::Sender<IpcCommand>,
    ) -> Result<(), WayclipError> {
        if let Some(combo) = user_settings.shortcuts.save_clip_controller.clone()
            && let Err(e) = self.controller.start(combo, command_sender.clone())
        {
            log::error!("Controller keybind failed: {e}");
        }

        let trigger = user_settings.shortcuts.save_clip.clone();
        let sender = command_sender.clone();
        let active = Arc::clone(&self.active_desktop);
        let cancel_token = self.cancel_token.clone();
        let handle = Handle::current();

        tokio::spawn(async move {
            let mut backoff = std::time::Duration::from_secs(2);

            loop {
                if cancel_token.is_cancelled() {
                    return;
                }

                if let Err(e) =
                    tokio::task::spawn_blocking(SessionVariable::import_from_user_manager).await
                {
                    log::warn!("session variable import task failed: {e}");
                }

                let trigger = trigger.clone();
                let sender = sender.clone();
                let handle = handle.clone();
                let token = cancel_token.clone();

                let result = tokio::task::spawn_blocking(move || {
                    let mut desktop = DesktopManager::new(trigger, token)?;
                    desktop.create_auto_bind(&sender, handle)?;
                    Ok::<DesktopManager, WayclipError>(desktop)
                })
                .await;

                match result {
                    Ok(Ok(desktop)) => {
                        if cancel_token.is_cancelled() {
                            // drop -> unregister
                            return;
                        }
                        log::info!(
                            "Desktop keybind registered successfully ({:?})",
                            desktop.desktop
                        );
                        *active.lock() = Some(desktop);
                        return;
                    }
                    Ok(Err(e)) => log::warn!("Desktop keybind registration failed: {e}"),
                    Err(e) => log::warn!("Desktop keybind registration task failed: {e}"),
                }

                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(15));
            }
        });

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), WayclipError> {
        self.controller.stop();
        self.cancel_token.cancel();

        let desktop = self.active_desktop.lock().take();
        match desktop {
            Some(mut desktop) => tokio::task::spawn_blocking(move || desktop.remove_auto_bind())
                .await
                .map_err(|e| WayclipError::Validation(e.to_string().into()))?,
            None => Ok(()),
        }
    }
}
