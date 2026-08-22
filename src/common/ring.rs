use gstreamer::ClockTime;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use wayclip_core::models::error::WayclipError;

// This file responsible for storing data and making the circular ring buffer.
// works both linux & windows so no specific calls except for like gstreamer

const DEFAULT_BACKWARDS_THRESHOLD_SECONDS: u64 = 1;

#[derive(Clone, Debug)]
pub enum ContentType {
    Video,
    Audio,
}

impl std::fmt::Display for ContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentType::Video => f.write_str("video"),
            ContentType::Audio => f.write_str("audio"),
        }
    }
}

pub type Payload = gstreamer::Buffer;

#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub data: Payload,
    pub pts: ClockTime,
    pub dts: Option<ClockTime>,
    pub duration: Option<ClockTime>,
    pub is_keyframe: bool,
}

impl EncodedFrame {
    pub fn new(
        data: Payload,
        pts: ClockTime,
        dts: Option<ClockTime>,
        duration: Option<ClockTime>,
        is_keyframe: bool,
    ) -> Self {
        Self {
            data,
            pts,
            dts,
            duration,
            is_keyframe,
        }
    }
}

#[derive(Debug)]
pub struct SaveData {
    pub video_frames: Vec<EncodedFrame>,
    pub audio_frames: Vec<EncodedFrame>,
    pub video_caps: gstreamer::Caps,
    pub audio_caps: Option<gstreamer::Caps>,
    // basically if video & audio frames are misaligned. this is offset of audio relative to video
    // (their first frames)
    pub sync_offset_ms: i64,
    pub duration: ClockTime,
}

#[derive(Clone, Debug)]
pub struct LogRateLimiter {
    pub interval: Duration,
    pub last: Option<Instant>,
}

impl LogRateLimiter {
    pub fn allow(&mut self) -> bool {
        let now = Instant::now();
        match self.last {
            Some(last) if now.duration_since(last) < self.interval => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

#[derive(Debug)]
pub struct RingBuffer {
    pub video_frames: VecDeque<EncodedFrame>,
    pub audio_frames: VecDeque<EncodedFrame>,

    pub video_caps: Option<gstreamer::Caps>,
    pub audio_caps: Option<gstreamer::Caps>,

    pub video_first_pts: Option<ClockTime>,
    pub audio_first_pts: Option<ClockTime>,

    pub video_start_instant: Option<Instant>,
    pub audio_start_instant: Option<Instant>,

    pub video_pts_offset_ns: i64,
    pub audio_pts_offset_ns: i64,

    pub awaiting_video_resync: bool,
    pub awaiting_audio_resync: bool,

    pub video_resync_reference: Option<ClockTime>,
    pub audio_resync_reference: Option<ClockTime>,

    // these lwk only should be used in debug...
    // TODO: remove or add the cfg thingy
    pub arriving_video_limiter: LogRateLimiter,
    pub arriving_audio_limiter: LogRateLimiter,
    pub evicted_frames_limiter: LogRateLimiter,

    pub max_duration: ClockTime,
}

impl RingBuffer {
    pub fn new(max_duration: ClockTime) -> Self {
        let rate_limiter = LogRateLimiter {
            interval: Duration::from_secs(1),
            last: None,
        };
        Self {
            video_frames: VecDeque::new(),
            audio_frames: VecDeque::new(),
            video_caps: None,
            audio_caps: None,
            video_first_pts: None,
            audio_first_pts: None,
            video_start_instant: None,
            audio_start_instant: None,
            max_duration,
            video_pts_offset_ns: 0,
            audio_pts_offset_ns: 0,
            awaiting_video_resync: false,
            awaiting_audio_resync: false,
            video_resync_reference: None,
            audio_resync_reference: None,
            arriving_video_limiter: rate_limiter.clone(),
            arriving_audio_limiter: rate_limiter.clone(),
            evicted_frames_limiter: rate_limiter.clone(),
        }
    }

    pub fn push_video_frame(&mut self, frame: EncodedFrame) -> Result<(), WayclipError> {
        self.video_frames.push_back(frame);
        self.evict_old_frames()
    }

    pub fn push_audio_frame(&mut self, frame: EncodedFrame) -> Result<(), WayclipError> {
        self.audio_frames.push_back(frame);
        self.evict_old_frames()
    }

    pub fn begin_resync(&mut self) {
        self.video_resync_reference = self.video_frames.back().map(|f| f.pts);
        self.audio_resync_reference = self.audio_frames.back().map(|f| f.pts);
        self.awaiting_video_resync = true;
        self.awaiting_audio_resync = true;

        log::debug!(
            "resync added references, video_ref: {:?} audio_ref: {:?}",
            self.video_resync_reference,
            self.audio_resync_reference
        );
    }

    pub fn evict_old_frames(&mut self) -> Result<(), WayclipError> {
        loop {
            if self.video_frames.len() <= 1 {
                break;
            }

            let front_pts = self
                .video_frames
                .front()
                .ok_or_else(|| WayclipError::Ring("No front video frame".into()))?
                .pts
                .saturating_sub(self.video_first_pts.unwrap_or(ClockTime::ZERO));

            let back_pts = self
                .video_frames
                .back()
                .ok_or_else(|| WayclipError::Ring("No back video frame".into()))?
                .pts
                .saturating_sub(self.video_first_pts.unwrap_or(ClockTime::ZERO));

            #[cfg(debug_assertions)]
            {
                if self.evicted_frames_limiter.allow() {
                    log::trace!(
                        "front_pts: {}ms, back_pts: {}ms ({} log/s)",
                        front_pts.mseconds(),
                        back_pts.mseconds(),
                        self.evicted_frames_limiter.interval.as_secs()
                    );
                }
            }

            if let Some(diff) = back_pts.checked_sub(front_pts) {
                if diff >= self.max_duration {
                    self.video_frames.pop_front();

                    if let Some(oldest_video) = self.video_frames.front() {
                        let video_pts = oldest_video
                            .pts
                            .saturating_sub(self.video_first_pts.unwrap_or(ClockTime::ZERO));

                        if let Some(audio_first) = self.audio_first_pts {
                            while let Some(audio_frame) = self.audio_frames.front() {
                                let audio_pts_relative =
                                    audio_frame.pts.saturating_sub(audio_first);

                                if audio_pts_relative
                                    < video_pts.saturating_sub(ClockTime::from_mseconds(100))
                                {
                                    self.audio_frames.pop_front();
                                } else {
                                    break;
                                }
                            }
                        }

                        #[cfg(debug_assertions)]
                        {
                            let before_count = self.audio_frames.len();
                            let after_count = self.audio_frames.len();
                            if before_count != after_count && self.evicted_frames_limiter.allow() {
                                log::trace!(
                                    "Evicted {} audio frames | oldest video PTS: {}ms | ({} log/s)",
                                    before_count - after_count,
                                    video_pts.mseconds(),
                                    self.evicted_frames_limiter.interval.as_secs()
                                );
                            }
                        }
                    }
                } else {
                    break;
                }
            } else {
                let backwards_jump = front_pts.checked_sub(back_pts).unwrap_or(ClockTime::ZERO);

                if backwards_jump > ClockTime::from_seconds(DEFAULT_BACKWARDS_THRESHOLD_SECONDS) {
                    log::warn!(
                        "True PTS clock reset detected (jumped back {}ms) — clearing ring buffer",
                        backwards_jump.mseconds()
                    );
                    let latest = self
                        .video_frames
                        .pop_back()
                        .ok_or_else(|| WayclipError::Ring("No pop_back".into()))?;
                    self.video_frames.clear();
                    self.video_frames.push_back(latest);
                    self.audio_frames.clear();
                    return Ok(());
                } else {
                    self.video_frames.pop_back();
                    break;
                }
            }
        }

        let audio_boundary = if let (Some(newest), Some(oldest)) =
            (self.audio_frames.back(), self.audio_frames.front())
        {
            Some((newest.pts, oldest.pts))
        } else {
            None
        };

        if let Some((newest_pts, oldest_pts)) = audio_boundary {
            let diff = newest_pts.saturating_sub(oldest_pts);

            if diff >= self.max_duration {
                while self.audio_frames.len() > 1 {
                    let front_pts = self
                        .audio_frames
                        .front()
                        .ok_or_else(|| WayclipError::Ring("No front audio frame".into()))?
                        .pts;
                    if newest_pts.saturating_sub(front_pts) >= self.max_duration {
                        self.audio_frames.pop_front();
                    } else {
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    pub fn get_snapshot(&self) -> Result<SaveData, WayclipError> {
        let video_caps = self.video_caps.clone().ok_or_else(|| {
            WayclipError::Ring("No video caps — pipeline may not have started yet".into())
        })?;

        if self.video_frames.is_empty() {
            return Err(WayclipError::Ring(
                "Ring buffer is empty — nothing to save".into(),
            ));
        }

        let first_pts = self.video_first_pts.unwrap_or(ClockTime::ZERO);

        log::debug!(
            "Ring buffer before drain: {} video frames, {} audio frames",
            self.video_frames.len(),
            self.audio_frames.len()
        );

        let first_keyframe_pos = self
            .video_frames
            .iter()
            .position(|f| f.is_keyframe)
            .unwrap_or(0);

        if first_keyframe_pos > 0 {
            log::debug!(
                "Skipping {} frames before first keyframe",
                first_keyframe_pos
            );
        }

        let video_frames: Vec<EncodedFrame> = self
            .video_frames
            .iter()
            .skip(first_keyframe_pos)
            .cloned()
            .collect();

        let video_start_pts = video_frames
            .iter()
            .map(|f| f.pts)
            .min()
            .unwrap_or(ClockTime::ZERO)
            .saturating_sub(first_pts);

        let video_end_pts = video_frames
            .iter()
            .map(|f| f.pts)
            .max()
            .unwrap_or(ClockTime::ZERO)
            .saturating_sub(first_pts);

        log::debug!(
            "Video timestamp range: {}ms to {}ms (span: {}ms)",
            video_start_pts.mseconds(),
            video_end_pts.mseconds(),
            video_end_pts.saturating_sub(video_start_pts).mseconds()
        );

        let threshold_start = video_start_pts.saturating_sub(ClockTime::from_mseconds(100));
        let threshold_end = video_end_pts.saturating_add(ClockTime::from_mseconds(100));

        let audio_first_pts = self.audio_first_pts.unwrap_or(ClockTime::ZERO);

        let audio_frames: Vec<EncodedFrame> = self
            .audio_frames
            .iter()
            .filter(|frame| {
                let pts = frame.pts.saturating_sub(audio_first_pts);
                pts >= threshold_start && pts <= threshold_end
            })
            .cloned()
            .collect();

        let sync_offset_ms = match (self.video_start_instant, self.audio_start_instant) {
            (Some(v), Some(a)) if (a >= v) => a.duration_since(v).as_millis() as i64,
            (Some(v), Some(a)) => -(v.duration_since(a).as_millis() as i64),
            _ => {
                log::warn!("missing starting insants, audio may be de-synced");
                0
            }
        };

        let duration = video_end_pts.saturating_sub(video_start_pts);

        log::debug!(
            "Draining {} video frames, {} audio frames, duration of {}s ({}ms offset)",
            video_frames.len(),
            audio_frames.len(),
            duration.seconds(),
            sync_offset_ms
        );

        Ok(SaveData {
            video_frames,
            audio_frames,
            video_caps,
            audio_caps: self.audio_caps.clone(),
            duration,
            sync_offset_ms,
        })
    }
}
