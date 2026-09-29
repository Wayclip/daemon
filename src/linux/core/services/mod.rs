use std::time::Instant;
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

use crate::{
    common::misc::discord::{DiscordPresenceManager, DiscordPresenceState},
    linux::discovery::Discovery,
};

pub struct DaemonServices {
    discord: Option<DiscordPresenceManager>,
    discovery: Option<Discovery>,

    last_tick: Instant,
}

impl DaemonServices {
    pub fn new(user_settings: &UserSettings) -> Result<Self, WayclipError> {
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
            last_tick: Instant::now(),
        })
    }
}
