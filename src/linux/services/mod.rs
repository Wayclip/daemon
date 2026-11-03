use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use chrono::Utc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wayclip_core::{
    models::{clips::games::GameRegistry, error::WayclipError},
    settings::UserSettings,
};

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

// Perhaps in future, we might have to also update other things on run-time. Then i will make a new
// module, for now keep it all here.
static UPDATER_STARTED: AtomicBool = AtomicBool::new(false);

pub struct DaemonServices {
    pub discovery: Option<Discovery>,
    discord: Option<(DiscordPresenceManager, Instant)>,
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

                Some((
                    DiscordPresenceManager::new(state),
                    Instant::now() - Duration::from_secs(60),
                ))
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
        let mut session_changed = false;

        if let Some(ref mut disc) = self.discovery
            && let Some(new_game) = disc.poll_changed()
        {
            log::info!("Game changed to: {:?}", new_game.as_ref().map(|g| &g.name));

            current_session.tick(new_game, None);
            session_changed = true;
        }

        let is_recently_saved = current_session
            .last_clip
            .as_ref()
            .is_some_and(|l| Utc::now().signed_duration_since(l.timestamp).num_seconds() < 15);

        let now = Instant::now();
        let refresh_needed = self.discord.as_ref().map_or(false, |d| {
            now.duration_since(d.1) >= Duration::from_secs(30)
        });

        if (session_changed || refresh_needed)
            && let Some(ref mut d) = self.discord
        {
            if is_recently_saved {
                d.0.set_saving(current_session.clone());
            } else {
                d.0.set_recording(current_session.clone());
            }

            d.1 = now;
        }

        Ok(())
    }

    pub fn saving(&mut self, current_session: CurrentSession) -> Result<(), WayclipError> {
        if let Some(ref d) = self.discord {
            d.0.set_saving(current_session);
        }

        Ok(())
    }

    pub fn start_services(
        &mut self,
        user_settings: &UserSettings,
        command_sender: &mpsc::Sender<IpcCommand>,
        current_session: &mut CurrentSession,
    ) -> Result<(), WayclipError> {
        // every 12h (not that user will for sure spend 12h)
        if !UPDATER_STARTED.swap(true, Ordering::Relaxed) {
            GameRegistry::spawn_periodic_updater(Duration::from_secs(12 * 60 * 60));
        }
        self.tray.start(command_sender);
        self.keybinds
            .setup_keybinds(user_settings, command_sender)?;
        // active it once, so even if no game is playing, discord status shows up
        if let Some(ref d) = self.discord {
            d.0.set_recording(current_session.clone());
        }
        Ok(())
    }

    pub async fn stop_services(&mut self) -> Result<(), WayclipError> {
        self.tray.stop().await;
        self.keybinds.stop()
    }
}
