use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

use crate::{
    common::misc::discord::{DiscordPresenceManager, DiscordPresenceState},
    linux::{
        ipc::commands::IpcCommand,
        services::{discovery::Discovery, keybinds::KeybindsService, tray::TrayManager},
        session::CurrentSession,
    },
};

pub mod discovery;
pub mod keybinds;
pub mod tray;

pub struct DaemonServices {
    pub discovery: Option<Discovery>,
    discord: Option<DiscordPresenceManager>,
    keybinds: KeybindsService,
    tray: TrayManager,
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
            tray: TrayManager::new(user_settings.tray.clone(), cancel_token.clone()),
            keybinds: KeybindsService::new(user_settings)?,
        })
    }

    pub fn update(&mut self, current_session: &mut CurrentSession) -> Result<(), WayclipError> {
        if let Some(ref mut disc) = self.discovery {
            if let Some(new_game) = disc.poll_changed() {
                log::info!("Game changed to: {:?}", new_game.as_ref().map(|g| &g.name));
                current_session.game = new_game;
                if let Some(ref d) = self.discord {
                    d.set_recording(current_session.clone());
                }
            }
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
