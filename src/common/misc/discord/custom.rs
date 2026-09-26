use crate::common::misc::extra::{FormatMode, WayclipTime, WayclipTimeTimezone};
use chrono::{DateTime, Utc};
use discord_rich_presence::activity::{Activity, ActivityType};
use url::Url;
use wayclip_core::{
    models::clips::games::ClipsGames,
    settings::{
        UserSettings,
        discovery::{CustomDiscordRichPresence, CustomDiscordRichPresenceActivityType},
    },
};

#[derive(Clone)]
pub struct CustomDiscordPresenceManager {
    state: String,
    details: String,
    activity_type: ActivityType,
}

impl From<CustomDiscordRichPresence> for CustomDiscordPresenceManager {
    fn from(value: CustomDiscordRichPresence) -> Self {
        Self {
            state: value.state,
            details: value.details,
            // We cannot implement a From<> for a 'foreign' enum, even if its technically ours
            activity_type: match value.activity_type {
                CustomDiscordRichPresenceActivityType::Playing => ActivityType::Playing,
                CustomDiscordRichPresenceActivityType::Watching => ActivityType::Watching,
                CustomDiscordRichPresenceActivityType::Listening => ActivityType::Listening,
                CustomDiscordRichPresenceActivityType::Competing => ActivityType::Competing,
            },
        }
    }
}

impl CustomDiscordPresenceManager {
    /// Method to build the activity struct using the custom configuration
    pub fn get_activity(&self, current_session: &CurrentSession) -> Activity {
        Activity::new()
            .state(self.state.clone())
            .details(self.parse_details(current_session))
            .activity_type(self.activity_type.clone())
    }

    /// This method will use the list defined in the struct and replace certain substrings with
    /// propper values
    fn parse_details(&self, current_session: &CurrentSession) -> String {
        let mut details = self.details.clone();

        // Time-related
        details = details.replace(
            "%time%",
            &WayclipTime::new(WayclipTimeTimezone::UTC).now_formatted_string(FormatMode::Default),
        );
        details = details.replace(
            "%started_at%",
            &WayclipTime::new(WayclipTimeTimezone::UTC)
                .format_timestamp(current_session.started_at.timestamp(), FormatMode::Default),
        );
        details = details.replace(
            "%playing_for%",
            &WayclipTime::new(WayclipTimeTimezone::UTC).get_time_delta_formatted(
                current_session.started_at.timestamp(),
                FormatMode::Default,
            ),
        );
        if let Some(ref last_clip) = current_session.last_clip {
            details = details.replace(
                "%last_clip%",
                &WayclipTime::new(WayclipTimeTimezone::UTC)
                    .format_timestamp(last_clip.timestamp.timestamp(), FormatMode::Default),
            )
        }

        // Session-related
        if let Some(game) = current_session.game {
            details = details.replace("%game%", &game.to_string());
        }
        let video_settings = &current_session.user_settings.recording.video;
        details = details.replace(
            "%buffer_length%",
            &video_settings.length_seconds.to_string(),
        );
        details = details.replace("%fps%", &video_settings.fps.to_string());
        details = details.replace("%res%", &video_settings.resolution.to_string());
        details = details.replace("%session_clips%", &current_session.clips.to_string());

        // TODO: Possibly instead of leaving raw string, but user is not logged in, we can either
        // put 'None' or leave an empty string ''
        if let Some(ref user_session) = current_session.user_session {
            details = details.replace("%username%", &user_session.username);
        }

        details
    }
}

/// TODO: MOVE
/// Struct responsible for the current session of wayclip. If an error occurs and daemon restarts,
/// this data will get persisted.
///
/// This data will be updated throughout the session and is used for various things, such as logs,
/// discord status (if enable), clip titles and more
#[derive(Clone, Debug)]
pub struct CurrentSession {
    pub game: Option<ClipsGames>,
    pub clips: u32,
    pub last_clip: Option<LastClipInfo>,
    pub started_at: DateTime<Utc>,
    pub user_settings: UserSettings,
    pub user_session: Option<UserSessionInfo>,
}

/// Will contain the current information about the user. Should be updated/fetched from
/// Users::get_me() [which caches it] some every N minutes or possibly when calling a discord status
/// update?
#[derive(Clone, Debug)]
pub struct UserSessionInfo {
    pub username: String,
    pub id: String,
    pub url: Url,
}

/// A Struct to be used inside of CurrentSession to handle information about the last saved clip.
/// Using this we can extract the name, URL (if uploaded -> can be handled if say auto-upload is
/// on) & timestamp
#[derive(Clone, Debug)]
pub struct LastClipInfo {
    pub timestamp: DateTime<Utc>,
    pub name: String,
    pub url: Option<Url>,
}

impl CurrentSession {
    pub fn new(user_settings: UserSettings, user_session: Option<UserSessionInfo>) -> Self {
        Self {
            game: None,
            clips: 0,
            last_clip: None,
            started_at: WayclipTime::get_utc(),
            user_settings,
            user_session,
        }
    }

    // TODO:
    // In the future, consider making this method fetch the UserSessionInfo by itself?
    pub async fn tick(&mut self, game: Option<ClipsGames>, user_session: Option<UserSessionInfo>) {
        self.game = game;
        self.user_session = user_session;
    }

    pub fn new_clip(&mut self, name: String, url: Option<Url>) {
        self.clips += 1;
        self.last_clip = Some(LastClipInfo {
            timestamp: WayclipTime::get_utc(),
            name,
            url,
        });
    }
}
