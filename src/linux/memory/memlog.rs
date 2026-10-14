use std::{fmt::Display, sync::Arc, time::Instant};

use parking_lot::Mutex;

use crate::common::video::ring::RingBuffer;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Heap {
    pub arena: u64,
    pub other_anon: u64,
}

impl Heap {
    pub fn read() -> Self {
        let Ok(smaps) = std::fs::read_to_string("/proc/self/smaps") else {
            return Self::default();
        };

        let mut out = Self::default();
        let mut name: Option<&str> = None;
        let mut anon = 0u64;

        let mut flush = |name: Option<&str>, anon: u64| {
            if anon == 0 {
                return;
            }
            if name == Some("[anon:mimalloc]") {
                out.arena += anon;
            } else {
                out.other_anon += anon;
            }
        };

        for line in smaps.lines() {
            if line.starts_with(|c: char| c.is_ascii_hexdigit()) && line.contains('-') {
                flush(name, anon);
                name = line.split_whitespace().last();
                anon = 0;
            } else if let Some(rest) = line.strip_prefix("Anonymous:") {
                anon = rest
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0)
                    * 1000;
            }
        }
        flush(name, anon);

        out
    }
}

/// The process own view of residency (/proc)
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Residency {
    pub rss: u64,
    pub anonymous: u64,
    pub private_dirty: u64,
    pub rss_anon: u64,
    pub rss_file: u64,
    pub rss_shmem: u64,
    pub anom_huge_pages: u64,
}

impl Residency {
    pub fn read() -> Self {
        let rollup = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let value = |text: &str, key: &str| -> u64 {
            text.lines()
                .find_map(|line| {
                    let rest = line.strip_prefix(key)?.strip_prefix(':')?;
                    rest.split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                        .map(|kb| kb * 1000)
                })
                .unwrap_or(0)
        };

        Self {
            rss: value(&rollup, "Rss"),
            anonymous: value(&rollup, "Anonymous"),
            private_dirty: value(&rollup, "Private_Dirty"),
            anom_huge_pages: value(&rollup, "AnonHugePages"),
            rss_anon: value(&status, "RssAnon"),
            rss_file: value(&status, "RssFile"),
            rss_shmem: value(&status, "RssShmem"),
        }
    }
}

/// One sample of everything needed to attribute growth.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub elapsed_secs: u64,
    pub residency: Residency,
    pub heap: Heap,
    pub video_frames: usize,
    pub audio_frames: usize,
    pub ring_bytes: usize,
    /// Frames per second actually delivered to the ring over the last sample interval
    pub video_fps: f64,
    pub audio_fps: f64,
    pub dropped: u64,
    pub video_flowing: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Counters {
    video: u64,
    audio: u64,
    at: Option<Instant>,
}

impl Display for Sample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let m = |b: u64| b as f64 / 1_000_000.0;
        let (r, h) = (&self.residency, &self.heap);
        write!(
            f,
            "t={:>6}s rss={:.0}M rss_anon={:.0}M rss_file={:.0}M rss_shmem={:.0}M dirty={:.0}M \
             mimalloc={:.0}M other_anon={:.0}M thp={:.0}M ring={:.1}M v={} a={} \
             vfps={:.2} afps={:.2} flow={} drop={}",
            self.elapsed_secs,
            m(r.rss),
            m(r.rss_anon),
            m(r.rss_file),
            m(r.rss_shmem),
            m(r.private_dirty),
            m(h.arena),
            m(h.other_anon),
            m(r.anom_huge_pages),
            m(self.ring_bytes as u64),
            self.video_frames,
            self.audio_frames,
            self.video_fps,
            self.audio_fps,
            if self.video_flowing { "yes" } else { "NO" },
            self.dropped,
        )
    }
}

impl Sample {
    pub fn capture(
        started: Instant,
        ring: &Arc<Mutex<RingBuffer>>,
        prev: &mut Counters,
        target_fps: f64,
        dropped: u64,
    ) -> Self {
        let (ring_len, audio_len, video_total, audio_total, ring_bytes) = {
            let ring = ring.lock();
            (
                ring.video_frames.len(),
                ring.audio_frames.len(),
                ring.video_frames_received,
                ring.audio_frames_received,
                ring.total_buffer_bytes,
            )
        };

        let now = Instant::now();
        let (video_fps, audio_fps) = match prev.at {
            Some(last) => {
                let secs = now.duration_since(last).as_secs_f64().max(0.001);
                (
                    video_total.saturating_sub(prev.video) as f64 / secs,
                    audio_total.saturating_sub(prev.audio) as f64 / secs,
                )
            }
            None => (0.0, 0.0),
        };
        *prev = Counters {
            video: video_total,
            audio: audio_total,
            at: Some(now),
        };

        Self {
            elapsed_secs: started.elapsed().as_secs(),
            residency: Residency::read(),
            heap: Heap::read(),
            video_frames: ring_len,
            audio_frames: audio_len,
            ring_bytes,
            video_fps,
            audio_fps,
            dropped,
            video_flowing: video_fps >= (target_fps - 2.0).max(1.0),
        }
    }
}
