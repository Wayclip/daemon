use crate::common::video::ring::data::{EncodedFrame, SaveData};
use gstreamer::ClockTime;
use std::{
    collections::VecDeque,
    io::{self, IsTerminal, Write},
    time::Instant,
};
use wayclip_core::models::error::WayclipError;

pub mod data;

const DEFAULT_BACKWARDS_THRESHOLD_SECONDS: u64 = 1;

/// This object is the ring buffer. It will actually hold all the frames in the memory, along with
/// caps, pts and start times to make sure everything is in sync. New frames are pushed into a
/// VecDeque, which pops the back once length exceeds maximum capacity
#[derive(Debug)]
pub struct RingBuffer {
    // The frames for video & audio
    pub video_frames: VecDeque<EncodedFrame>,
    pub audio_frames: VecDeque<EncodedFrame>,
    // Capabilities (caps) of the video/audio stream so we can re-encode properly
    pub video_caps: Option<gstreamer::Caps>,
    pub audio_caps: Option<gstreamer::Caps>,
    // The first PTS
    pub video_first_pts: Option<ClockTime>,
    pub audio_first_pts: Option<ClockTime>,
    // Start timestamp
    pub video_start_instant: Option<Instant>,
    pub audio_start_instant: Option<Instant>,
    // Timestamp of most recent frame (we need this to detect stalls inside pipeline without
    // watchdog)
    pub video_last_instant: Option<Instant>,
    pub audio_last_instant: Option<Instant>,
    // The offset for PTS
    pub video_pts_offset_ns: i64,
    pub audio_pts_offset_ns: i64,
    // Flag to synchronise video/audio
    pub awaiting_video_resync: bool,
    pub awaiting_audio_resync: bool,
    // Pts to synchronise to
    pub video_resync_reference: Option<ClockTime>,
    pub audio_resync_reference: Option<ClockTime>,
    // Maximum duration fo the ring buffer, after which frames will be popped
    pub max_duration: ClockTime,

    // Tracking variables for debug statistics
    pub video_frames_received: u64,
    pub audio_frames_received: u64,
    pub total_buffer_bytes: usize,
}

impl RingBuffer {
    pub fn new(max_duration: ClockTime) -> Self {
        Self {
            video_frames: VecDeque::new(),
            audio_frames: VecDeque::new(),
            video_caps: None,
            audio_caps: None,
            video_first_pts: None,
            audio_first_pts: None,
            video_start_instant: None,
            audio_start_instant: None,
            video_last_instant: None,
            audio_last_instant: None,
            max_duration,
            video_pts_offset_ns: 0,
            audio_pts_offset_ns: 0,
            awaiting_video_resync: false,
            awaiting_audio_resync: false,
            video_resync_reference: None,
            audio_resync_reference: None,
            video_frames_received: 0,
            audio_frames_received: 0,
            total_buffer_bytes: 0,
        }
    }

    /// Method allowing us to push a new video frame
    pub fn push_video_frame(&mut self, frame: EncodedFrame) -> Result<(), WayclipError> {
        // If the frame recieved is the first one (since we didnt have first PTS stored), uupdate
        // the timestamp
        if self.video_first_pts.is_none() {
            self.video_first_pts = Some(frame.pts);
            self.video_start_instant = Some(Instant::now());
        }

        self.video_last_instant = Some(Instant::now());

        // Increment debug counters
        self.video_frames_received += 1;
        self.total_buffer_bytes += frame.payload.size();

        // A debug line tool allowing us to visually see all the info in real-time
        #[cfg(debug_assertions)]
        self.debug_line(&frame);

        // Now, we can call the method to push into VecDeque & then evict_old_frames which checks
        // every frame using timestamps
        self.video_frames.push_back(frame);
        self.evict_old_frames()
    }

    // We can use terminal technique to always print to same line
    fn debug_line(&self, frame: &EncodedFrame) {
        if !io::stderr().is_terminal() {
            return;
        }

        // Get resolution string
        let res = self.resolution_str();
        // Calculate the total ring buffer size in MB
        let total_mb = self.total_buffer_bytes as f64 / (1000.0 * 1000.0);

        // FWe can get the duration, and how much we have used up
        let buf_dur = self.buffer_duration(&self.video_frames);
        let buf_s = buf_dur.mseconds() as f64 / 1000.0;
        let max_s = self.max_duration.mseconds() as f64 / 1000.0;

        let elapsed_s = self
            .video_start_instant
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        let avg_fps = if elapsed_s > 0.0 {
            self.video_frames_received as f64 / elapsed_s
        } else {
            0.0
        };

        // AV drift can show how unsynced our audio and vidoe is
        let av_drift = self.calculate_av_drift_str();
        // also add in a tag if the frame recieved is a keyframe
        let kf_tag = if frame.is_keyframe { "[KEY]" } else { "     " };
        let size_kb = frame.payload.size() as f64 / 1000.0;

        // write to same line
        eprint!(
            "\r{:<9} | Buf: {:>4.1}/{:>4.1}s ({:>5.1} MB | V:{:<4} A:{:<4}) | {:>4.1} fps | A/V: {:>7} | In: {} {:>5.1} KB\x1b[K",
            res,
            buf_s,
            max_s,
            total_mb,
            self.video_frames.len(),
            self.audio_frames.len(),
            avg_fps,
            av_drift,
            kf_tag,
            size_kb
        );
        let _ = io::stderr().flush();
    }

    // Same method as for video, but instead we are handing auido frames. Both call same
    // evict_old_frames method
    pub fn push_audio_frame(&mut self, frame: EncodedFrame) -> Result<(), WayclipError> {
        if self.audio_first_pts.is_none() {
            self.audio_first_pts = Some(frame.pts);
            self.audio_start_instant = Some(Instant::now());
        }

        self.audio_last_instant = Some(Instant::now());

        self.audio_frames_received += 1;
        self.total_buffer_bytes += frame.payload.size();
        self.audio_frames.push_back(frame);
        self.evict_old_frames()
    }

    // Re-Sync works by grabbing onto the last known frame and recording its PTS. This PTS is then
    // used to compare the new start pos and we can adjust correctly
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

    // Quick little method to just gra the front most frame and extract its PTS
    fn get_pts(&self) -> Result<(ClockTime, ClockTime), WayclipError> {
        Ok((
            self.video_frames
                .front()
                .ok_or_else(|| WayclipError::Ring("No front video frame".into()))?
                .pts,
            self.video_frames
                .back()
                .ok_or_else(|| WayclipError::Ring("No back video frame".into()))?
                .pts,
        ))
    }

    // Evict old frames uses timestamps to remove old/expired frames
    pub fn evict_old_frames(&mut self) -> Result<(), WayclipError> {
        // We keep looping whilst we have frames, or until an internal condition breaks the loop
        while self.video_frames.len() > 1 {
            // Extract front-most PTS
            let (front_pts, back_pts) = self.get_pts()?;

            // Non-monotonic clock jump detection
            if front_pts > back_pts {
                let jump = front_pts.saturating_sub(back_pts);
                // If the jump is too big, we destroy ourselves, reset all data, and push this frame
                // as a new one. Quite harsh, but overwise we can have large jumps in video
                if jump > ClockTime::from_seconds(DEFAULT_BACKWARDS_THRESHOLD_SECONDS) {
                    log::warn!(
                        "True PTS clock reset detected (jumped back {}ms) — clearing ring buffer",
                        jump.mseconds()
                    );
                    let latest = self.video_frames.pop_back().unwrap();
                    self.reset();
                    self.total_buffer_bytes = latest.payload.size();
                    self.video_frames.push_back(latest);
                    return Ok(());
                }

                // Even if not a large jump, we will pop the back, get its size and remove it from
                // total for debug info
                if let Some(popped) = self.video_frames.pop_back() {
                    self.total_buffer_bytes = self
                        .total_buffer_bytes
                        .saturating_sub(popped.payload.size());
                }

                // Since jump happened, we will exit loop here
                break;
            }

            // Now, our difference between back and fron PTS is below the maximum duration, we will
            // also exit
            if back_pts.saturating_sub(front_pts) < self.max_duration {
                break;
            }

            // Then, since we are over the time limit, we pop the front, and subtract the size from
            // our counter
            if let Some(popped) = self.video_frames.pop_front() {
                self.total_buffer_bytes = self
                    .total_buffer_bytes
                    .saturating_sub(popped.payload.size());
            }

            // Prune audio lagging behind the oldest video frame (-100ms tolerance)
            if let (Some(oldest_video), Some(audio_first)) =
                (self.video_frames.front(), self.audio_first_pts)
            {
                // get the threshold at which we gotta eliminate the audio frames by subtracting the
                // first pts from oldest frame and adding 100ms
                let video_pts = oldest_video
                    .pts
                    .saturating_sub(self.video_first_pts.unwrap_or(ClockTime::ZERO));
                let threshold = video_pts.saturating_sub(ClockTime::from_mseconds(100));

                // Go through each audio frame, compare, pop, update data
                while let Some(audio) = self.audio_frames.front() {
                    if audio.pts.saturating_sub(audio_first) < threshold {
                        if let Some(popped) = self.audio_frames.pop_front() {
                            self.total_buffer_bytes = self
                                .total_buffer_bytes
                                .saturating_sub(popped.payload.size());
                        }
                    } else {
                        break;
                    }
                }
            }
        }

        // Keep audio duration within bounds
        if let Some(newest) = self.audio_frames.back() {
            let newest_pts = newest.pts;
            while self.audio_frames.len() > 1
                && newest_pts.saturating_sub(self.audio_frames.front().unwrap().pts)
                    >= self.max_duration
            {
                if let Some(popped) = self.audio_frames.pop_front() {
                    self.total_buffer_bytes = self
                        .total_buffer_bytes
                        .saturating_sub(popped.payload.size());
                }
            }
        }

        Ok(())
    }

    // Get snapshot allows us to extract data without bringing down the whole buffer, meaning we
    // keep it rolling.
    pub fn get_snapshot(&self) -> Result<SaveData, WayclipError> {
        // Video caps needed for re-encoding
        let video_caps = self.video_caps.clone().ok_or_else(|| {
            WayclipError::Ring("No video caps - pipeline may not have started yet".into())
        })?;

        if self.video_frames.is_empty() {
            return Err(WayclipError::Ring(
                "Ring buffer is empty - nothing to save".into(),
            ));
        }

        let first_pts = self.video_first_pts.unwrap_or(ClockTime::ZERO);

        log::debug!(
            "Ring buffer before drain: {} video frames, {} audio frames",
            self.video_frames.len(),
            self.audio_frames.len()
        );

        // We have to get data past the first keyframe, otherwise it will get corrupted. So find
        // first keyframe, remove everthing before that
        let keyframe_idx = self
            .video_frames
            .iter()
            .position(|f| f.is_keyframe)
            .unwrap_or(0);

        if keyframe_idx > 0 {
            log::debug!("Skipping {} frames before first keyframe", keyframe_idx);
        }

        let video_frames: Vec<EncodedFrame> = self
            .video_frames
            .iter()
            .skip(keyframe_idx)
            .cloned()
            .collect();

        // Acquire the true start and end PTS
        let video_start_pts = video_frames
            .first()
            .map(|f| f.pts)
            .unwrap_or(ClockTime::ZERO)
            .saturating_sub(first_pts);
        let video_end_pts = video_frames
            .last()
            .map(|f| f.pts)
            .unwrap_or(ClockTime::ZERO)
            .saturating_sub(first_pts);
        let duration = video_end_pts.saturating_sub(video_start_pts);

        log::debug!(
            "Video timestamp range: {}ms to {}ms (span: {}ms)",
            video_start_pts.mseconds(),
            video_end_pts.mseconds(),
            duration.mseconds()
        );

        // Add 100ms threshold to both start and end
        let threshold_start = video_start_pts.saturating_sub(ClockTime::from_mseconds(100));
        let threshold_end = video_end_pts.saturating_add(ClockTime::from_mseconds(100));
        let audio_first_pts = self.audio_first_pts.unwrap_or(ClockTime::ZERO);

        // Filter out the frames so they fit the new video length
        let audio_frames: Vec<EncodedFrame> = self
            .audio_frames
            .iter()
            .filter(|frame| {
                let pts = frame.pts.saturating_sub(audio_first_pts);
                pts >= threshold_start && pts <= threshold_end
            })
            .cloned()
            .collect();

        // Get the offset needed & return all the data
        let sync_offset_ms = self.calculate_sync_offset_ms();

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

    fn buffer_duration(&self, frames: &VecDeque<EncodedFrame>) -> ClockTime {
        match (frames.front(), frames.back()) {
            (Some(front), Some(back)) => back.pts.saturating_sub(front.pts),
            _ => ClockTime::ZERO,
        }
    }

    // Get the captured resolution (not the one stored in settings)
    fn resolution_str(&self) -> String {
        self.video_caps
            .as_ref()
            .and_then(|caps| {
                let s = caps.structure(0)?;
                let w = s.get::<i32>("width").ok()?;
                let h = s.get::<i32>("height").ok()?;
                Some(format!("{}x{}", w, h))
            })
            .unwrap_or_else(|| "?x?".to_string())
    }

    fn calculate_av_drift_str(&self) -> String {
        match (self.video_frames.back(), self.audio_frames.back()) {
            (Some(v), Some(a)) => {
                let v_pts = v
                    .pts
                    .saturating_sub(self.video_first_pts.unwrap_or(ClockTime::ZERO));
                let a_pts = a
                    .pts
                    .saturating_sub(self.audio_first_pts.unwrap_or(ClockTime::ZERO));

                if a_pts >= v_pts {
                    format!("+{}ms", (a_pts.saturating_sub(v_pts)).mseconds())
                } else {
                    format!("-{}ms", (v_pts.saturating_sub(a_pts)).mseconds())
                }
            }
            _ => "syncing".to_string(),
        }
    }

    // Small offset calculation function
    fn calculate_sync_offset_ms(&self) -> i64 {
        match (self.video_start_instant, self.audio_start_instant) {
            (Some(v), Some(a)) if a >= v => a.duration_since(v).as_millis() as i64,
            (Some(v), Some(a)) => -(v.duration_since(a).as_millis() as i64),
            _ => {
                log::warn!("missing starting instants, audio may be de-synced");
                0
            }
        }
    }

    fn reset(&mut self) {
        self.video_frames.clear();
        self.audio_frames.clear();
        self.video_first_pts = None;
        self.audio_first_pts = None;
        self.video_start_instant = None;
        self.audio_start_instant = None;
        self.video_pts_offset_ns = 0;
        self.audio_pts_offset_ns = 0;
        self.awaiting_video_resync = false;
        self.awaiting_audio_resync = false;
        self.video_resync_reference = None;
        self.audio_resync_reference = None;
        self.video_frames_received = 0;
        self.audio_frames_received = 0;
        self.total_buffer_bytes = 0;
    }
}
