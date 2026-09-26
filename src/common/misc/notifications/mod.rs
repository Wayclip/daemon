use strum_macros::FromRepr;
use wayclip_core::models::error::WayclipError;
use wayclip_core::settings::notifications::NotificationSettings;
use wayclip_core::t;

use crate::common::misc::notifications::sound::NotificationSound;

pub mod sound;

pub struct NotificationManager;

//pub const DBUS_NOTIFICATION_DESTINATION: &str = "org.freedesktop.Notifications";
//pub const DBUS_NOTIFICATION_PATH: &str = "/org/freedesktop/Notifications";
//static DBUS_CONNECTION: OnceLock<Connection> = OnceLock::new();

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

impl From<Urgency> for notify_rust::Urgency {
    fn from(value: Urgency) -> Self {
        match value {
            Urgency::Normal => Self::Normal,
            Urgency::Critical => Self::Critical,
        }
    }
}

impl NotificationEvent {
    pub fn get_summary(&self) -> String {
        match self {
            Self::DaemonStop => t!("notification.daemon_stop.summary"),
            Self::DaemonStart => t!("notification.daemon_start.summary"),
            Self::SaveError => t!("notification.save_error.summary"),
            Self::SaveSuccess => t!("notification.save_success.summary"),
            Self::Test => t!("notification.test.summary"),
        }
    }

    pub fn get_body(&self, content: String) -> String {
        match self {
            Self::SaveSuccess => t!("notification.save_success.body", content = content),
            Self::SaveError => t!("notification.save_error.body", content = content),
            Self::DaemonStart => t!("notification.daemon_start.body"),
            Self::DaemonStop => t!("notification.daemon_stop.body"),
            Self::Test => t!("notification.test.body", content = content),
        }
    }

    pub fn get_icon(&self) -> &'static str {
        "wayclip"
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
        NotificationSound::process_audio_event(&event, &settings);
        NotificationManager::process_message_event(&event, &settings, content);

        Ok(())
    }

    fn process_message_event(
        event: &NotificationEvent,
        settings: &NotificationSettings,
        content: String,
    ) {
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
    }

    // dont directly expose as pub, has to be done as blocking...
    fn send_notification(event: NotificationEvent, content: String) -> Result<(), WayclipError> {
        let summary = event.get_summary();
        let body = event.get_body(content);
        let urgency = event.get_urgency();
        let icon = event.get_icon();
        let timeout_ms = event.get_timeout_ms();

        notify_rust::Notification::new()
            .appname("Wayclip")
            .summary(&summary)
            .urgency(urgency.into())
            .body(&body)
            .icon(icon)
            .timeout(notify_rust::Timeout::Milliseconds(timeout_ms as u32))
            .show()
            .map_err(|e| WayclipError::Validation(e.to_string().into()))?;

        //let conn = if let Some(conn) = DBUS_CONNECTION.get() {
        //    conn
        //} else {
        //    let conn = Connection::session()?;
        //    DBUS_CONNECTION.set(conn).ok();
        //    DBUS_CONNECTION.get().ok_or_else(|| {
        //        WayclipError::Validation("Failed to initialize D-Bus connection".into())
        //    })?
        //};

        //let mut hints: HashMap<&str, Value> = HashMap::new();
        //hints.insert("urgency", Value::U8(urgency as u8));

        //// https://specifications.freedesktop.org/notification-spec/latest/
        //conn.call_method(
        //    Some(DBUS_NOTIFICATION_DESTINATION),
        //    DBUS_NOTIFICATION_PATH,
        //    Some(DBUS_NOTIFICATION_DESTINATION),
        //    "Notify",
        //    &(
        //        "Wayclip",
        //        0u32, // make new notif
        //        icon,
        //        summary,
        //        body.as_str(),
        //        Vec::<&str>::new(), // empty actions
        //        hints,
        //        timeout_ms,
        //    ),
        //)?;

        Ok(())
    }
}
