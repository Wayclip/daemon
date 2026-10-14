use chrono::{DateTime, Utc};
use url::Url;
use wayclip_core::{models::clips::games::Game, settings::UserSettings};

use crate::common::misc::extra::WayclipTime;

/// Struct responsible for the current session of wayclip. If an error occurs and daemon restarts,
/// this data will get persisted.
///
/// This data will be updated throughout the session and is used for various things, such as logs,
/// discord status (if enable), clip titles and more
#[derive(Clone, Debug)]
pub struct CurrentSession {
    pub game: Option<Game>,
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
    pub fn tick(&mut self, game: Option<Game>, user_session: Option<UserSessionInfo>) {
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
