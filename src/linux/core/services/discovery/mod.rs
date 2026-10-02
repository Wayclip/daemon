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
            last_scan: Instant::now() - Duration::from_secs(10),
        })
    }

    pub fn poll_changed(&mut self) -> Option<Option<Game>> {
        if self.last_scan.elapsed() < Duration::from_secs(2) {
            return None;
        }
        self.last_scan = Instant::now();

        let candidates = self.scanner.scan().unwrap_or_default();
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
        match candidates.len() {
            0 => None,
            1 => Some(candidates.into_iter().next().unwrap().game),
            _ => {
                if let Some(f) = focused_pid {
                    let chain = Scanner::ancestors(f);
                    if let Some(c) = candidates.iter().find(|c| chain.contains(&c.pid)) {
                        return Some(c.game.clone());
                    }
                }
                if let Some(prev) = previous {
                    if let Some(c) = candidates.iter().find(|c| &c.game == prev) {
                        return Some(c.game.clone());
                    }
                }
                candidates
                    .into_iter()
                    .max_by_key(|c| c.start)
                    .map(|c| c.game)
            }
        }
    }
}
