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
    pub ring_pinned: usize,
}

impl Display for Sample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let m = |b: u64| b as f64 / 1_000_000.0;
        let (r, a) = (&self.residency, &self.malloc);
        write!(
            f,
            "t={:>6}s rss={:.0}M rss_anon={:.0}M rss_file={:.0}M rss_shmem={:.0}M dirty={:.0}M \
             beyond_malloc={:.0}M arena={:.0}M inuse={:.0}M free_in_arena={:.0}M frag={:.1}% \
             mmap={:.0}M thp={:.0}M ring={:.1}M ring_pinned={:.1}M v={} a={}",
            self.elapsed_secs,
            m(r.rss),
            m(r.rss_anon),
            m(r.rss_file),
            m(r.rss_shmem),
            m(r.private_dirty),
            m(r.anonymous_beyond_malloc(a)),
            m(a.arena),
            m(a.uordblks),
            m(a.fordblks),
            a.fragmentation() * 100.0,
            m(a.hblkhd),
            m(r.anom_huge_pages),
            m(self.ring_bytes as u64),
            m(self.ring_pinned as u64),
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
            ring_pinned: ring
                .video_frames
                .iter()
                .chain(ring.audio_frames.iter())
                .map(|f| {
                    (0..f.payload.n_memory())
                        .map(|i| f.payload.peek_memory(i).maxsize())
                        .sum::<usize>()
                })
                .sum(),
        }
    }
}
