use discord_rich_presence::{
    DiscordIpc, DiscordIpcClient,
    activity::{Activity, ActivityType, Timestamps},
};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

const DISCORD_CLIENT_ID: &str = "1416659610416316466";

enum PresenceCommand {
    Recording { game: Option<String> },
    // Idle
}

pub struct DiscordPresenceManager {
    tx: mpsc::Sender<PresenceCommand>,
}

impl Default for DiscordPresenceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscordPresenceManager {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<PresenceCommand>();

        std::thread::spawn(move || {
            let mut client: Option<DiscordIpcClient> = None;
            let mut started_at: Option<i64> = None;

            for command in rx {
                match command {
                    //PresenceCommand::Idle => {
                    //    started_at = None;
                    //    if let Some(discord) = client.as_mut()
                    //        && let Err(error) = discord.clear_activity()
                    //    {
                    //        log::debug!("Could not clear Discord activity: {error}");
                    //        client = None;
                    //    }
                    //}
                    PresenceCommand::Recording { game } => {
                        let recording_started_at = *started_at.get_or_insert_with(|| {
                            SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map(|duration| duration.as_secs() as i64)
                                .unwrap_or(0)
                        });

                        if client.is_none() {
                            log::debug!("Connecting to Discord IPC");

                            let mut new_client = DiscordIpcClient::new(DISCORD_CLIENT_ID);

                            if let Err(error) = new_client.connect() {
                                log::debug!(
                                    "Discord is not running or IPC connection failed: {error}"
                                );

                                continue;
                            }

                            client = Some(new_client);

                            log::debug!("Connected to Discord IPC");
                        }

                        let details = game.as_deref().unwrap_or("Desktop");

                        let activity = Activity::new()
                            .state("Recording")
                            .details(details)
                            .activity_type(ActivityType::Playing)
                            .timestamps(Timestamps::new().start(recording_started_at));

                        log::debug!(
                            "Sending Discord activity: details={details:?}, \
                             started_at={recording_started_at}"
                        );

                        let set_activity_failed = if let Some(discord) = client.as_mut() {
                            discord.set_activity(activity).is_err()
                        } else {
                            true
                        };

                        if set_activity_failed {
                            log::warn!("Failed to set Discord activity");

                            client = None;
                        }
                    }
                }
            }

            if let Some(mut discord) = client.take()
                && let Err(error) = discord.close()
            {
                log::debug!("Failed to close Discord IPC connection: {error}");
            }
        });

        Self { tx }
    }

    pub fn set_recording(&self, game: Option<String>) {
        let _ = self.tx.send(PresenceCommand::Recording { game });
    }

    //pub fn set_idle(&self) {
    //    let _ = self.tx.send(PresenceCommand::Idle);
    //}
}
