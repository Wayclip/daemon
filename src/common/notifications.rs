use rodio::{Decoder, DeviceSinkBuilder, Player};
use std::{collections::HashMap, fs::File, path::PathBuf, sync::OnceLock};
use strum_macros::FromRepr;
use wayclip_core::models::error::WayclipError;
use wayclip_core::settings::notifications::NotificationSettings;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

// TODO: This file violates the common principle, since we are accessing DBUS as a direct method of
// playing sound.
pub struct NotificationManager;

pub const DBUS_NOTIFICATION_DESTINATION: &str = "org.freedesktop.Notifications";
pub const DBUS_NOTIFICATION_PATH: &str = "/org/freedesktop/Notifications";
// so we persist connection
static DBUS_CONNECTION: OnceLock<Connection> = OnceLock::new();

#[derive(Debug, Clone)]
pub enum NotificationEvent {
    SaveSuccess,
    SaveError,
    DaemonStart,
    DaemonStop,
    Test,
}

#[derive(Debug, Clone, Copy, FromRepr)]
pub enum Urgency {
    Normal = 1,
    Critical = 2,
}

impl NotificationEvent {
    pub fn get_summary(&self) -> &str {
        match self {
            Self::DaemonStop => "Daemon has stopped",
            Self::DaemonStart => "Daemon has started",
            Self::SaveError => "Error saving clip",
            Self::SaveSuccess => "Saved new clip",
            Self::Test => "A test notification",
        }
    }

    pub fn get_body(&self, content: String) -> String {
        match self {
            Self::SaveSuccess => format!("A new clip was saved succesfully: {}", content),
            Self::SaveError => format!("Error occurred while saving clip: {}", content),
            Self::DaemonStart => {
                "The Wayclip daemon was started and is currently recording".to_string()
            }
            Self::DaemonStop => "The Wayclip daemon has stopped recording".to_string(),
            Self::Test => format!("Test: {}", content),
        }
    }

    pub fn get_icon(&self) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap_or(&PathBuf::default())
            .join("branding")
            .join("svgs")
            .join("wayclip.svg")
    }

    pub fn get_timeout_ms(&self) -> i32 {
        match self {
            Self::DaemonStop | Self::DaemonStart => 1500,
            Self::SaveError => 4000,
            Self::SaveSuccess => 1000,
            Self::Test => 500,
        }
    }

    pub fn get_urgency(&self) -> Urgency {
        match self {
            Self::SaveError => Urgency::Critical,
            _ => Urgency::Normal,
        }
    }
}

#[derive(Debug, Clone)]
pub enum NotificationSound {
    Success,
    Error,
}

impl NotificationSound {
    pub fn get_sound_path(&self) -> PathBuf {
        let sound_name = match self {
            Self::Success => "success.wav",
            Self::Error => "error.wav",
        };

        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("static")
            .join("sounds")
            .join(sound_name)
    }
}

impl NotificationManager {
    pub async fn test_notification(
        event: NotificationEvent,
        content: String,
    ) -> Result<(), WayclipError> {
        tokio::task::spawn_blocking(move || Self::send_notification(event, content)).await??;

        Ok(())
    }

    pub fn process_event(
        event: NotificationEvent,
        settings: NotificationSettings,
        content: String,
    ) -> Result<(), WayclipError> {
        log::info!("Notification Event Triggered: {:?}", event);

        let play_audio = match event {
            NotificationEvent::SaveError if settings.sounds.on_save_error => {
                Some(NotificationSound::Error)
            }
            NotificationEvent::SaveSuccess if settings.sounds.on_save_success => {
                Some(NotificationSound::Success)
            }
            _ => None,
        };

        if let Some(sound) = play_audio {
            tokio::task::spawn_blocking(move || {
                if let Err(e) = Self::play_sound(sound) {
                    log::error!("Audio Error: {}", e);
                }
            });
        }

        let send_msg = match event {
            NotificationEvent::SaveError => settings.message.on_save_error,
            NotificationEvent::SaveSuccess => settings.message.on_save_success,
            NotificationEvent::DaemonStart => settings.message.on_daemon_start,
            NotificationEvent::DaemonStop => settings.message.on_daemon_stop,
            _ => false,
        };

        if send_msg {
            let event_clone = event.clone();
            let content_clone = content.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(e) = Self::send_notification(event_clone, content_clone) {
                    log::error!("Notification Error: {}", e);
                }
            });
        }

        Ok(())
    }

    fn play_sound(sound: NotificationSound) -> Result<(), WayclipError> {
        let path = sound.get_sound_path();
        log::debug!("Playing sound: {:?}", path);

        let device_sink = DeviceSinkBuilder::open_default_sink()?;

        let player = Player::connect_new(device_sink.mixer());
        let file = File::open(&path)?;
        let source = Decoder::try_from(file)?;

        player.append(source);

        player.sleep_until_end();

        Ok(())
    }

    // dont directly expose as pub, has to be done as blocking...
    fn send_notification(event: NotificationEvent, content: String) -> Result<(), WayclipError> {
        let summary = event.get_summary();
        let body = event.get_body(content);
        let urgency = event.get_urgency();
        let icon_path = event.get_icon();
        let icon = icon_path
            .to_str()
            .ok_or_else(|| WayclipError::Validation("Could not convert logo to str".into()))?;
        let timeout_ms = event.get_timeout_ms();

        let conn = if let Some(conn) = DBUS_CONNECTION.get() {
            conn
        } else {
            let conn = Connection::session()?;
            DBUS_CONNECTION.set(conn).ok();
            DBUS_CONNECTION.get().ok_or_else(|| {
                WayclipError::Validation("Failed to initialize D-Bus connection".into())
            })?
        };

        let mut hints: HashMap<&str, Value> = HashMap::new();
        hints.insert("urgency", Value::U8(urgency as u8));

        // https://specifications.freedesktop.org/notification-spec/latest/
        conn.call_method(
            Some(DBUS_NOTIFICATION_DESTINATION),
            DBUS_NOTIFICATION_PATH,
            Some(DBUS_NOTIFICATION_DESTINATION),
            "Notify",
            &(
                "Wayclip",
                0u32, // make new notif
                icon,
                summary,
                body.as_str(),
                Vec::<&str>::new(), // empty actions
                hints,
                timeout_ms,
            ),
        )?;

        Ok(())
    }
}
