use crate::common::misc::notifications::NotificationEvent;
use rodio::{Decoder, DeviceSinkBuilder, Player};
use std::io::Cursor;
use wayclip_core::{
    models::error::WayclipError,
    settings::notifications::{NotificationSettings, SOUND_SAVE_ERROR, SOUND_SAVE_SUCCESS},
};

#[derive(Debug, Clone)]
pub enum NotificationSound {
    Success,
    Error,
}

impl NotificationSound {
    pub fn process_audio_event(event: &NotificationEvent, settings: &NotificationSettings) {
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
                if let Err(e) = sound.play() {
                    log::error!("Audio Error: {}", e);
                }
            });
        }
    }

    fn get_bytes(&self) -> &'static [u8] {
        match self {
            Self::Success => SOUND_SAVE_SUCCESS,
            Self::Error => SOUND_SAVE_ERROR,
        }
    }

    fn play(&self) -> Result<(), WayclipError> {
        let bytes = self.get_bytes();
        let device_sink = DeviceSinkBuilder::open_default_sink()?;

        let player = Player::connect_new(device_sink.mixer());
        let source = Decoder::try_from(Cursor::new(bytes))?;

        player.append(source);
        player.sleep_until_end();
        Ok(())
    }
}
