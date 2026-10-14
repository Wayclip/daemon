use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
};
use wayclip_core::models::error::WayclipError;

use crate::linux::services::discovery::Game;

const CANDIDATE_ROOTS: [&str; 5] = [
    ".local/share/Steam",
    ".steam/root",
    ".steam/steam",
    ".var/app/com.valvesoftware.Steam/.local/share/Steam",
    "snap/steam/common/.local/share/Steam",
];
const CANDIDATE_VDF: [&str; 2] = ["config/libraryfolders.vdf", "steamapps/libraryfolders.vdf"];

pub struct SteamLib {
    library_paths: Vec<PathBuf>,
    cache: HashMap<u32, Game>,
}

impl SteamLib {
    pub fn new() -> Result<Self, WayclipError> {
        let mut lib = Self {
            cache: HashMap::new(),
            library_paths: Vec::new(),
        };

        lib.refresh()?;
        Ok(lib)
    }

    // Finds the steam installation roots and parses libraryfolders.vdf to access all the data
    // metadata
    fn refresh(&mut self) -> Result<(), WayclipError> {
        let home = dirs::home_dir()
            .ok_or_else(|| WayclipError::NotFound("No home directory found".into()))?;

        let mut libraries = Vec::new();

        for root in CANDIDATE_ROOTS {
            let path = home.join(root);
            for vdf in CANDIDATE_VDF {
                let path = path.join(vdf);
                if path.is_file() {
                    libraries.extend(self.parse_library_dir(&path));
                }
            }
            if path.is_dir() {
                libraries.push(path)
            }
        }

        libraries.sort();
        libraries.dedup();
        self.library_paths = libraries;

        Ok(())
    }

    pub fn resolve(&mut self, appid: u32) -> Result<Option<Game>, WayclipError> {
        if let Some(g) = self.cache.get(&appid) {
            return Ok(Some(g.clone()));
        }

        let manifest_filename = format!("appmanifest_{}.acf", appid);

        for library in &self.library_paths {
            let manifest_path = library.join("steamapps").join(&manifest_filename);
            if !manifest_path.is_file() {
                continue;
            }

            if let Some(name) = self.extract_manifest_name(&manifest_path)? {
                let game = Game::new(name, Some(appid));
                self.cache.insert(appid, game.clone());
                return Ok(Some(game));
            }
        }

        Ok(None)
    }

    fn parse_library_dir(&self, path: &PathBuf) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        let Ok(file) = File::open(path) else {
            return dirs;
        };

        let reader = BufReader::new(file);
        for l in reader.lines().map_while(Result::ok) {
            if let Some(p) = l.split('"').nth(3).map(PathBuf::from) {
                if p.is_dir() {
                    dirs.push(p)
                }
            }
        }
        dirs
    }

    fn extract_manifest_name(&self, path: &PathBuf) -> Result<Option<String>, WayclipError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);

        Ok(reader.lines().map_while(Result::ok).find_map(|l| {
            let s = l.trim();
            if s.starts_with("\"name\"") {
                s.split('"')
                    .nth(3)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            } else {
                None
            }
        }))
    }
}
