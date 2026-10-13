use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use strum_macros::Display;

use crate::{
    common::gst::bus::CoreEvent,
    linux::{DaemonCore, engine::gstreamer::CaptureMode},
};

const RECOVERY_TIMEOUT: Duration = Duration::from_secs(30);
const BACKOFF: [Duration; 6] = [
    Duration::from_secs(0),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(30),
];
const HEALTHY_RESET: Duration = Duration::from_secs(30);
const FLAP_WINDOW: Duration = Duration::from_secs(600);
const FLAP_LIMIT: usize = 4;
const VIDEO_MEMORY_MARKERS: [&str; 4] = [
    "out_of_memory",
    "out of memory",
    "failed to upload buffer",
    "not_supported_device",
];

/// Why a recovery was requested
#[derive(Debug, Clone, PartialEq, Eq, Display)]
pub enum Problem {
    #[strum(serialize = "video stalled")]
    Stalled,
    #[strum(serialize = "guard dropped every frame")]
    Degraded,
    #[strum(serialize = "unexpected EOS")]
    Eos,
    #[strum(to_string = "{msg}")]
    Pipeline { msg: String, video_memory: bool },
    #[strum(to_string = "previous recovery failed: {0}")]
    RecoveryFailed(String),
}

impl Problem {
    pub fn from_event(event: CoreEvent) -> Self {
        match event {
            CoreEvent::PipelineEos => Self::Eos,
            CoreEvent::PipelineError(msg) => {
                let lower = msg.to_ascii_lowercase();
                let video_memory = VIDEO_MEMORY_MARKERS.iter().any(|m| lower.contains(m));
                Self::Pipeline { msg, video_memory }
            }
        }
    }

    fn is_video_memory(&self) -> bool {
        matches!(
            self,
            Self::Pipeline {
                video_memory: true,
                ..
            }
        )
    }
}

#[derive(Default)]
pub struct Recovery {
    /// Number of recoveries in a row
    attempt: usize,
    due: Option<(Instant, Problem)>,
    healthy_since: Option<Instant>,
    /// Start times of recent recoveries
    recent: VecDeque<Instant>,
}

impl Recovery {
    pub fn pending(&self) -> bool {
        self.due.is_some()
    }

    pub fn schedule(&mut self, problem: Problem) {
        if self.due.is_some() {
            return;
        }

        self.recent.retain(|t| t.elapsed() < FLAP_WINDOW);
        let mut delay = BACKOFF[self.attempt.min(BACKOFF.len() - 1)];
        if self.recent.len() >= FLAP_LIMIT {
            delay = delay.max(BACKOFF[BACKOFF.len() - 1]);
            log::error!(
                "{} recoveries in the last {}min",
                self.recent.len(),
                FLAP_WINDOW.as_secs() / 60
            );
        }

        log::warn!(
            "pipeline problem ({problem}); recovery #{} in {}s",
            self.attempt + 1,
            delay.as_secs()
        );
        self.due = Some((Instant::now() + delay, problem));
    }

    /// Call every tick
    pub fn track(&mut self, flowing: bool) {
        if !flowing {
            self.healthy_since = None;
            return;
        }
        let since = *self.healthy_since.get_or_insert_with(Instant::now);
        if self.attempt > 0 && since.elapsed() >= HEALTHY_RESET {
            log::info!("video healthy again");
            self.attempt = 0;
        }
    }

    /// Returns `(attempt, problem)`
    fn take_due(&mut self) -> Option<(usize, Problem)> {
        let (_, problem) = self.due.take_if(|(due, _)| Instant::now() >= *due)?;
        let attempt = self.attempt;
        self.attempt += 1;
        self.recent.push_back(Instant::now());
        Some((attempt, problem))
    }

    fn succeeded(&mut self) {
        self.healthy_since = None;
    }
}

enum RecoveryAction {
    FullRebuild,
    MemoryDowngrade(String),
    QuickRestart,
    PipelineRestart,
}

// Dont quite like this too, but will saves the mod.rs from looking like hell..
impl DaemonCore {
    fn select_action(&self, attempt: usize, problem: Problem, stalled: bool) -> RecoveryAction {
        let n = attempt + 1;

        if !self.engine.has_portal() || attempt >= 2 || (stalled && attempt >= 1) {
            log::warn!("recovery #{n}: full rebuild ({problem})");
            RecoveryAction::FullRebuild
        } else if attempt == 1 {
            log::warn!("recovery #{n}: rebuilding on system memory ({problem})");
            let reason = problem.to_string();
            RecoveryAction::MemoryDowngrade(reason)
        } else if problem.is_video_memory() {
            log::warn!("recovery #{n}: in-place restart ({problem})");
            RecoveryAction::QuickRestart
        } else {
            log::warn!("recovery #{n}: rebuilding pipeline ({problem})");
            RecoveryAction::PipelineRestart
        }
    }

    pub(super) fn track_health(&mut self) {
        let flowing = self.engine.video_flowing();
        self.recovery.track(flowing);
    }

    pub(super) async fn run_due_recovery(&mut self) {
        let Some((attempt, problem)) = self.recovery.take_due() else {
            return;
        };

        let settings = self.current_session.user_settings.clone();
        let sender = self.event_channel.0.clone();
        let stalled = problem == Problem::Stalled;
        let n = attempt + 1;

        let res = match self.select_action(attempt, problem, stalled) {
            RecoveryAction::FullRebuild => {
                if !stalled || attempt >= 2 {
                    self.engine.capture_mode = CaptureMode::SystemMemory;
                }
                tokio::time::timeout(RECOVERY_TIMEOUT, self.engine.recover(&settings, sender)).await
            }
            RecoveryAction::MemoryDowngrade(reason) => {
                tokio::time::timeout(
                    RECOVERY_TIMEOUT,
                    self.engine
                        .downgrade_to_system_memory(&settings, sender, &reason),
                )
                .await
            }
            RecoveryAction::QuickRestart => {
                tokio::time::timeout(RECOVERY_TIMEOUT, self.engine.retry(sender)).await
            }
            RecoveryAction::PipelineRestart => {
                tokio::time::timeout(
                    RECOVERY_TIMEOUT,
                    self.engine.restart_pipeline(&settings, sender),
                )
                .await
            }
        };

        let failure = match res {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e.to_string()),
            Err(_) => Some(format!("timed out after {}s", RECOVERY_TIMEOUT.as_secs())),
        };

        match failure {
            None => {
                while self.event_channel.1.try_recv().is_ok() {}
                self.recovery.succeeded();
                log::info!("recovery #{n} done");
            }
            Some(e) => {
                log::error!("recovery #{n} failed: {e}");
                self.recovery.schedule(Problem::RecoveryFailed(e));
            }
        }
    }

    pub(super) async fn retry_dmabuf(&mut self) {
        let settings = self.current_session.user_settings.clone();
        let sender = self.event_channel.0.clone();
        let res = tokio::time::timeout(
            RECOVERY_TIMEOUT,
            self.engine.retry_dmabuf(&settings, sender),
        )
        .await;

        match res {
            Ok(Ok(())) => while self.event_channel.1.try_recv().is_ok() {},
            other => {
                let reason = match other {
                    Ok(Err(e)) => e.to_string(),
                    _ => "timed out".to_string(),
                };
                log::warn!("DMABuf retry failed to start ({reason}). staying on system memory");
                self.engine.capture_mode = CaptureMode::SystemMemory;
                self.recovery.schedule(Problem::RecoveryFailed(reason));
            }
        }
    }
}
