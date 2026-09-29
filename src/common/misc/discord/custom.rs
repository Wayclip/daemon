use crate::{
    common::misc::extra::{FormatMode, WayclipTime, WayclipTimeTimezone},
    linux::core::session::CurrentSession,
};
use discord_rich_presence::activity::{Activity, ActivityType};
use wayclip_core::settings::discovery::{
    CustomDiscordRichPresence, CustomDiscordRichPresenceActivityType,
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
    pub fn new(state: String, details: String, activity_type: ActivityType) -> Self {
        Self {
            state,
            details,
            activity_type,
        }
    }

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
