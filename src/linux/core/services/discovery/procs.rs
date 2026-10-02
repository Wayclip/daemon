use crate::linux::core::services::discovery::{Game, db::ExecutableDb, steam::SteamLib};
use std::{
    collections::{HashMap, HashSet},
    fs,
};
use wayclip_core::models::error::WayclipError;

#[derive(Clone, Debug)]
pub struct Candidate {
    pub pid: u32,
    pub start: u64,
    pub game: Game,
}

struct Verdict {
    start: u64,
    tries: u8,
    game: Option<Game>,
}

pub struct Scanner {
    seen: HashMap<u32, Verdict>,
    db: ExecutableDb,
    steam: SteamLib,
}

impl Scanner {
    pub fn new() -> Result<Self, WayclipError> {
        Ok(Self {
            seen: HashMap::new(),
            db: ExecutableDb::load()?,
            steam: SteamLib::new()?,
        })
    }

    pub fn scan(&mut self) -> Result<Vec<Candidate>, WayclipError> {
        let mut alive = HashSet::new();

        for e in fs::read_dir("/proc")?.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse().ok()) else {
                continue;
            };
            let Some((start, _)) = Self::read_stat_fields(pid) else {
                continue;
            };
            alive.insert(pid);

            if let Some(v) = self.seen.get(&pid) {
                if v.start == start && (v.game.is_some() || v.tries >= 2) {
                    continue;
                }
            }

            let tries = self.seen.get(&pid).map_or(0, |v| v.tries) + 1;
            let game = self.classify(pid)?;
            self.seen.insert(pid, Verdict { start, tries, game });
        }

        self.seen.retain(|p, _| alive.contains(p));
        Ok(self
            .seen
            .iter()
            .filter_map(|(&pid, v)| {
                v.game.clone().map(|game| Candidate {
                    pid,
                    start: v.start,
                    game,
                })
            })
            .collect())
    }

    fn classify(&mut self, pid: u32) -> Result<Option<Game>, WayclipError> {
        let raw = fs::read(format!("/proc/{pid}/cmdline"))?;
        let args: Vec<_> = raw.split(|&b| b == 0).filter(|a| !a.is_empty()).collect();
        let Some(&first) = args.first() else {
            return Ok(None);
        };

        if Self::basename(first) == b"reaper" {
            let id = args
                .iter()
                .find_map(|a| a.strip_prefix(b"AppId="))
                .and_then(|a| std::str::from_utf8(a).ok()?.parse().ok())
                .ok_or_else(|| WayclipError::NotFound("No AppId".into()))?;
            return self.steam.resolve(id);
        }

        if let Some(g) = fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .and_then(|p| self.db.by_exe(p.file_name()?.to_str()?))
        {
            return Ok(Some(g));
        }

        Ok(args
            .iter()
            .take(3)
            .find_map(|&a| self.db.by_exe(std::str::from_utf8(Self::basename(a)).ok()?)))
    }

    pub fn read_stat_fields(pid: u32) -> Option<(u64, u32)> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let mut parts = stat.rsplit_once(") ")?.1.split_whitespace();
        let ppid = parts.nth(1)?.parse().ok()?;
        let start = parts.nth(17)?.parse().ok()?;
        Some((start, ppid))
    }

    pub fn ancestors(mut pid: u32) -> HashSet<u32> {
        let mut tree = HashSet::new();
        while pid > 1 && tree.insert(pid) {
            match Self::read_stat_fields(pid) {
                Some((_, ppid)) if ppid != pid => pid = ppid,
                _ => break,
            }
        }
        tree
    }

    fn basename(path: &[u8]) -> &[u8] {
        path.rsplit(|&b| b == b'/' || b == b'\\')
            .next()
            .unwrap_or(path)
    }
}
