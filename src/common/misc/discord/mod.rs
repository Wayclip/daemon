use crate::{
    common::misc::discord::custom::CustomDiscordPresenceManager,
    linux::core::session::{CurrentSession, LastClipInfo},
};
use discord_rich_presence::{
    DiscordIpc, DiscordIpcClient,
    activity::{Activity, ActivityType, Button, Timestamps},
};
use std::sync::mpsc;

pub mod custom;

const WAYCLIP_DISCORD_CLIENT_ID: &str = "1416659610416316466";

#[derive(Clone, Debug)]
enum PresenceCommand {
    RecordingEvent { current_session: CurrentSession },
    SavingEvent { current_session: CurrentSession },
}

#[derive(Clone)]
pub enum DiscordPresenceState {
    Custom(CustomDiscordPresenceManager),
    Default,
}

///  States:
///  (default)
///  1. Recording
///  2. Just saved clip
///
///  (custom)
///  3. Custom
///
///  Recording:
///  - Show how long
///  - Have counter & increment on save
///  - If logged in: Link user profile
///  - Button: Get Wayclip (promo button that user can disable)
///
///  Just saved a clip:
///  - Show length
///  - Get timestamp & display for ~3 min
///  - If uploaded: Link clip URL
///
///  Custom:
///  - State
///  - Details (with parsing)
///  - ActivityType
pub struct DiscordPresenceManager {
    tx: mpsc::Sender<PresenceCommand>,
    pub state: DiscordPresenceState,
}

impl DiscordPresenceManager {
    fn set_activity(client: &mut Option<DiscordIpcClient>, activity: Activity) {
        if client.is_none() {
            log::debug!("Connecting to Discord IPC");

            let mut new_client = DiscordIpcClient::new(WAYCLIP_DISCORD_CLIENT_ID);

            if let Err(error) = new_client.connect() {
                log::debug!("Discord is not running or IPC connection failed: {error}");
            }

            *client = Some(new_client);
            log::debug!("Connected to Discord IPC");
        }

        let set_activity_failed = if let Some(discord) = client.as_mut() {
            discord.set_activity(activity).is_err()
        } else {
            true
        };

        if set_activity_failed {
            log::warn!("Failed to set Discord activity");
            *client = None;
        }
    }

    fn get_saved_buttons(last_clip_info: Option<LastClipInfo>) -> Vec<Button<'static>> {
        let mut buttons = Vec::new();

        if let Some(clip_info) = last_clip_info
            && let Some(url) = clip_info.url
        {
            buttons.push(Button::new(clip_info.name, url.to_string()));
        }

        buttons
    }

    fn get_recording_buttons(
        promo_enabled: bool,
        user_session_info: Option<UserSessionInfo>,
    ) -> Vec<Button<'static>> {
        let mut buttons = Vec::new();

        if promo_enabled {
            buttons.push(
                // TODO: Fix hardcoded URL
                Button::new("Get Wayclip", "https://wayclip.com"),
            );
        }

        if let Some(user_session) = user_session_info {
            buttons.push(Button::new(
                format!("{}'s profile", user_session.username),
                user_session.url.to_string(),
            ));
        }

        buttons
    }

    pub fn new(state: DiscordPresenceState) -> Self {
        let (tx, rx) = mpsc::channel::<PresenceCommand>();

        let state_clone = state.clone();
        tokio::spawn(async move {
            let mut client: Option<DiscordIpcClient> = None;

            for command in rx {
                let activity = match command {
                    PresenceCommand::RecordingEvent { current_session } => match state_clone {
                        DiscordPresenceState::Default => Activity::new()
                            .state("Recording")
                            .details(
                                current_session
                                    .game
                                    .map(|g| g.to_string())
                                    .unwrap_or(String::from("Desktop")),
                            )
                            .activity_type(ActivityType::Playing)
                            // TODO: A way to disable promo
                            .buttons(Self::get_recording_buttons(
                                true,
                                current_session.user_session,
                            ))
                            .timestamps(
                                Timestamps::new().start(current_session.started_at.timestamp()),
                            ),
                        DiscordPresenceState::Custom(ref custom_config) => {
                            custom_config.get_activity(&current_session)
                        }
                    },
                    PresenceCommand::SavingEvent { current_session } => match state_clone {
                        DiscordPresenceState::Default => Activity::new()
                            .state("Saved clip")
                            .details("Just saved a new clip using Wayclip!")
                            .activity_type(ActivityType::Playing)
                            .buttons(Self::get_saved_buttons(current_session.last_clip)),
                        DiscordPresenceState::Custom(ref custom_config) => {
                            custom_config.get_activity(&current_session)
                        }
                    },
                };

                Self::set_activity(&mut client, activity);
            }

            if let Some(mut discord) = client.take()
                && let Err(error) = discord.close()
            {
                log::debug!("Failed to close Discord IPC connection: {error}");
            }
        });

        Self { tx, state }
    }

    //pub fn set_recording(&self, game: Option<String>) {
    //    let _ = self.tx.send(PresenceCommand::Recording { game });
    //}

    //pub fn set_idle(&self) {
    //    let _ = self.tx.send(PresenceCommand::Idle);
    //}
}
