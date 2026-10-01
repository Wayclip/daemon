use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

use crate::{
    common::misc::discord::{DiscordPresenceManager, DiscordPresenceState},
    linux::{
        core::{
            ipc::commands::IpcCommand,
            services::{keybinds::KeybindsService, tray::TrayManager},
            session::CurrentSession,
        },
        discovery::Discovery,
    },
};

pub mod keybinds;
pub mod tray;

pub struct DaemonServices {
    discord: Option<DiscordPresenceManager>,
    discovery: Option<Discovery>,
    keybinds: KeybindsService,
    tray: TrayManager,

    last_tick: Instant,
}

impl DaemonServices {
    pub fn new(
        user_settings: &UserSettings,
        cancel_token: CancellationToken,
    ) -> Result<Self, WayclipError> {
        let discovery = match user_settings.game_discovery.enabled {
            true => Some(Discovery::new()?),
            false => None,
        };

        let discord = match user_settings.game_discovery.discord_rich_presence.enabled {
            true => {
                let state = if let Some(custom) = user_settings
                    .game_discovery
                    .discord_rich_presence
                    .custom
                    .clone()
                {
                    DiscordPresenceState::Custom(custom.into())
                } else {
                    DiscordPresenceState::Default
                };

                Some(DiscordPresenceManager::new(state))
            }
            false => None,
        };

        Ok(Self {
            discord,
            discovery,
            tray: TrayManager::new(user_settings.tray.clone()),
            keybinds: KeybindsService::new(user_settings, cancel_token)?,
            last_tick: Instant::now(),
        })
    }

    pub fn update(&mut self, current_session: CurrentSession) -> Result<(), WayclipError> {
        if let Some(ref d) = self.discord {
            d.set_recording(current_session);
        }

        if let Some(ref mut d) = self.discovery {
            d.discover_game();
        }

        Ok(())
    }

    pub fn saving(&mut self, current_session: CurrentSession) -> Result<(), WayclipError> {
        if let Some(ref d) = self.discord {
            d.set_saving(current_session);
        }

        Ok(())
    }

    pub fn start_services(
        &mut self,
        user_settings: &UserSettings,
        command_sender: &mpsc::Sender<IpcCommand>,
    ) -> Result<(), WayclipError> {
        self.tray.start(command_sender);
        self.keybinds.setup_keybinds(user_settings, command_sender)
    }

    pub async fn stop_services(&mut self) -> Result<(), WayclipError> {
        self.tray.stop().await;
        self.keybinds.stop()
    }
}
