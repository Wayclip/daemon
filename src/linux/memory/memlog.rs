use std::{fmt::Display, sync::Arc, time::Instant};

use parking_lot::Mutex;

use crate::common::video::ring::RingBuffer;

/// data coming from `mallinfo2()`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MallInfo {
    /// Total bytes obtained from the system for the arenas
    pub arena: u64,
    /// In-use bytes
    pub uordblks: u64,
    /// Free bytes held inside the arenas
    pub fordblks: u64,
    /// Address space reserved for mmapped chunks
    pub hblkhd: u64,
    /// Number of said chunks
    pub hblks: u64,
}

impl MallInfo {
    /// Bytes glibc is responsible for (arena plus mmapped chunks)
    pub fn accounted(&self) -> u64 {
        self.arena + self.hblkhd
    }

    pub fn fragmentation(&self) -> f64 {
        if self.arena == 0 {
            return 0.0;
        }
        self.fordblks as f64 / self.arena as f64
    }

    pub fn read() -> Self {
        let mi = unsafe { libc::mallinfo2() };
        Self {
            arena: mi.arena as u64,
            uordblks: mi.uordblks as u64,
            fordblks: mi.fordblks as u64,
            hblkhd: mi.hblkhd as u64,
            hblks: mi.hblks as u64,
        }
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
                    rest.split_whitespace().next()?.parse().ok()
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

    /// Anonymous bytes glibc does not account for (driver mappings, dmabuf, thread stacks)
    pub fn anonymous_beyond_malloc(&self, mi: &MallInfo) -> u64 {
        self.anonymous.saturating_sub(mi.accounted())
    }
}

/// One sample of everything needed to attribute growth.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub elapsed_secs: u64,
    pub residency: Residency,
    pub malloc: MallInfo,
    pub video_frames: usize,
    pub audio_frames: usize,
    pub ring_bytes: usize,
}

impl Display for Sample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "t={:>6}s rss={:>7}M anon={:>7}M dirty={:>7}M anon_beyond_malloc={:>7}M \
             arena={:>7}M inuse={:>7}M free_in_arena={:>7}M frag={:>5.1}% mmap={:>6}M \
             rss_anon={:>7}M rss_file={:>6}M rss_shmem={:>5}M thp={:>5}M \
             ring={:>6}M v={} a={}",
            self.elapsed_secs,
            (self.residency.rss) / 1000,
            (self.residency.anonymous) / 1000,
            (self.residency.private_dirty) / 1000,
            (self.residency.anonymous_beyond_malloc(&self.malloc) / 1000),
            (self.malloc.arena) / 1000,
            (self.malloc.uordblks) / 1000,
            (self.malloc.fordblks) / 1000,
            self.malloc.fragmentation() * 100.0,
            (self.malloc.hblkhd) / 1000,
            (self.residency.rss_anon) / 1000,
            (self.residency.rss_file) / 1000,
            (self.residency.rss_shmem) / 1000,
            (self.residency.anom_huge_pages) / 1000,
            (self.ring_bytes as u64) / 1000,
            self.video_frames,
            self.audio_frames,
        )
    }
}

impl Sample {
    pub fn capture(started: Instant, ring: &Arc<Mutex<RingBuffer>>) -> Self {
        let ring = ring.lock();
        Self {
            elapsed_secs: started.elapsed().as_secs(),
            residency: Residency::read(),
            malloc: MallInfo::read(),
            video_frames: ring.video_frames.len(),
            audio_frames: ring.audio_frames.len(),
            ring_bytes: ring.total_buffer_bytes,
        }
    }
}
