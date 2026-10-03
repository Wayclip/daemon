use std::time::{Duration, Instant};
use wayclip_core::models::{clips::games::Game, error::WayclipError};

pub mod db;
pub mod focus;
pub mod procs;
pub mod steam;

use focus::WindowFocusScanner;
use procs::{Candidate, Scanner};

pub struct Discovery {
    scanner: Scanner,
    current_game: Option<Game>,
    last_scan: Instant,
}

impl Discovery {
    pub fn new() -> Result<Self, WayclipError> {
        Ok(Self {
            scanner: Scanner::new()?,
            current_game: None,
            last_scan: Instant::now()
                .checked_sub(Duration::from_secs(10))
                .unwrap_or_else(Instant::now),
        })
    }

    pub fn poll_changed(&mut self) -> Option<Option<Game>> {
        if self.last_scan.elapsed() < Duration::from_secs(2) {
            return None;
        }
        self.last_scan = Instant::now();

        let candidates = match self.scanner.scan() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("scan: {e}");
                return None;
            }
        };
        let focused = WindowFocusScanner::focused_pid();
        let next_game = Self::pick_active_game(candidates, focused, self.current_game.as_ref());

        if next_game != self.current_game {
            self.current_game = next_game.clone();
            Some(next_game)
        } else {
            None
        }
    }

    pub fn current_game(&self) -> Option<&Game> {
        self.current_game.as_ref()
    }

    fn pick_active_game(
        candidates: Vec<Candidate>,
        focused_pid: Option<u32>,
        previous: Option<&Game>,
    ) -> Option<Game> {
        // 1. Stickiness: if we already have a game and it's still running, keep it.
        if let Some(prev) = previous {
            if candidates.iter().any(|c| &c.game == prev) {
                return Some(prev.clone());
            }
        }

        // Separate Steam vs non-Steam candidates
        let (steam_candidates, other_candidates): (Vec<_>, Vec<_>) = candidates
            .into_iter()
            .partition(|c| c.game.steam_appid.is_some());

        // 2. Try to pick based on focused window + ancestors, preferring Steam games.
        if let Some(focused_pid) = focused_pid {
            let chain = Scanner::ancestors(focused_pid);

            // First, try Steam games in the ancestor chain
            if let Some(candidate) = steam_candidates.iter().find(|c| chain.contains(&c.pid)) {
                return Some(candidate.game.clone());
            }

            // Then, try any game in the ancestor chain
            if let Some(candidate) = other_candidates.iter().find(|c| chain.contains(&c.pid)) {
                return Some(candidate.game.clone());
            }
        }

        // 3. No focus-based match: prefer Steam games, then the oldest-running one.
        steam_candidates
            .into_iter()
            .max_by_key(|c| c.start)
            .map(|c| c.game)
            .or_else(|| {
                other_candidates
                    .into_iter()
                    .max_by_key(|c| c.start)
                    .map(|c| c.game)
            })
    }
}
