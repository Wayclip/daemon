use crate::common::misc::notifications::{NotificationEvent, NotificationManager};
use crate::common::video::SaveManager;
use crate::common::video::preview::PreviewManager;
use crate::linux::core::DaemonCore;
use crate::linux::core::types::DaemonStatus;
use chrono::Local;
use gstreamer::prelude::ElementExt;
use log::{error, info, warn};
use sd_notify::NotifyState;
use std::sync::Arc;
use wayclip_core::app::clips::query::ClipsQuery;
use wayclip_core::models::clips::local::LocalClip;
use wayclip_core::models::error::WayclipError;

impl DaemonCore {
    pub async fn rescan_games(&mut self) -> (String, f32) {
        self.discovery.discover_game();
        let game = self
            .discovery
            .confident_game()
            .map(|g| g.to_string())
            .unwrap_or_default();
        let confidence = self.discovery.confidence;
        (game, confidence)
    }
}
