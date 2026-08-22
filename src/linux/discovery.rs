use crate::linux::desktop::{DesktopEnvironmentManager, DesktopEnvironmentType, SessionType};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::Command,
    str,
    time::{Duration, Instant},
};
use wayclip_core::models::{clips::games::ClipsGames, error::WayclipError};

pub const STEAM_SCAN_MIN_INTERVAL: Duration = Duration::from_secs(60);
pub const CONFIDENCE_THRESHOLD: f32 = 0.5;
pub const APPID_CONFIDENCE: f32 = 0.75;
pub const MANIFEST_NAME_CONFIDENCE: f32 = 0.6;

#[derive(Clone, Debug, Default)]
pub struct Discovery {
    pub desktop_env: DesktopEnvironmentType,
    pub session_type: SessionType,
    pub game: Option<ClipsGames>,
    pub confidence: f32,
    pub last_steam_scan: Option<Instant>,
    pub last_steam_found: bool,
    pub active_window_title: Option<String>,
    pub active_window_process_name: Option<String>,
}

impl Discovery {
    pub fn new() -> Result<Self, WayclipError> {
        let (desktop_env, session_type) = DesktopEnvironmentManager::get_env_session()?;
        Ok(Self {
            desktop_env,
            session_type,
            ..Default::default()
        })
    }

    fn adjust_confidence(&mut self, delta: f32) {
        self.confidence = (self.confidence + delta).clamp(0.0, 1.0);
    }

    fn clear_detection(&mut self) {
        self.game = None;
        self.confidence = 0.0;
    }

    pub fn confident_game(&self) -> Option<ClipsGames> {
        if self.confidence >= CONFIDENCE_THRESHOLD {
            self.game.filter(|g| *g != ClipsGames::Unknown)
        } else {
            None
        }
    }

    pub fn discover_game(&mut self) {
        let window_ok = self.use_active_window().is_ok();

        let due_for_scan = self
            .last_steam_scan
            .is_none_or(|last| last.elapsed() >= STEAM_SCAN_MIN_INTERVAL);

        let steam_found = if due_for_scan {
            self.last_steam_scan = Some(Instant::now());
            let found = match self.use_active_steam_app() {
                Ok(found) => found,
                Err(e) => {
                    log::warn!("steam app detection failed: {e}");
                    false
                }
            };
            self.last_steam_found = found;
            found
        } else {
            self.last_steam_found
        };

        if !steam_found {
            if window_ok {
                self.match_window_title_process();
            } else {
                self.clear_detection();
            }
        }
    }

    fn match_window_title_process(&mut self) {
        if let Some(title) = self.active_window_title.clone()
            && let Some(process) = self.active_window_process_name.clone()
        {
            let mut p_lower = process.to_lowercase();

            if p_lower.ends_with(".exe") || p_lower.ends_with("-bin") {
                p_lower.truncate(p_lower.len() - 4);
            }

            let exact_ignore = [
                "chrome",
                "chromium",
                "firefox",
                "brave",
                "msedge",
                "vivaldi",
                "opera",
                "waterfox",
                "librewolf",
                "zen",
                "thorium",
                "discord",
                "vesktop",
                "webcord",
                "vlc",
                "mpv",
                "spotify",
                "code",
                "kitty",
                "alacritty",
                "wezterm",
                "dolphin",
                "nautilus",
                "thunar",
                "nemo",
                "obs",
                "obs64",
                "obs-studio",
                "steamwebhelper",
            ];

            let contains_ignore = [
                "google-chrome",
                "chromium-browser",
                "brave-browser",
                "microsoft-edge",
                "zen-alpha",
                "zen-beta",
                "zen-browser",
                "discord-canary",
                "discord-ptb",
                "org.mozilla.",
                "com.google.chrome",
                "com.brave.browser",
                "vlc",
                "thorium-browser",
                "com.obsproject.studio",
            ];

            let t_lower = title.to_lowercase();
            let browser_title_indicators = [
                " - google chrome",
                " - chromium",
                " - mozilla firefox",
                " - waterfox",
                " - librewolf",
                " - zen browser",
                " - brave",
                " - microsoft edge",
                " - vivaldi",
                " - opera",
                " - youtube",
                " - twitch",
                " - netflix",
                " - hulu",
                " - disney+",
                " - discord",
            ];

            if exact_ignore.contains(&p_lower.as_str())
                || contains_ignore.iter().any(|&c| p_lower.contains(c))
                || browser_title_indicators
                    .iter()
                    .any(|&s| t_lower.contains(s))
            {
                self.clear_detection();
                return;
            }

            let game_title = ClipsGames::from_title(&title);
            let game_process = ClipsGames::from_process_name(&process);

            let both_recognized =
                game_title != ClipsGames::Unknown && game_process != ClipsGames::Unknown;

            let resolved = if both_recognized && game_title == game_process {
                self.adjust_confidence(0.2);
                Some(game_title)
            } else if both_recognized {
                self.adjust_confidence(-0.2);
                Some(game_process)
            } else if game_process != ClipsGames::Unknown {
                self.adjust_confidence(0.1);
                Some(game_process)
            } else if game_title != ClipsGames::Unknown {
                self.adjust_confidence(0.1);
                Some(game_title)
            } else {
                let combined = format!("{title} {process}");
                let game_combined = ClipsGames::from_title(&combined);

                if game_combined != ClipsGames::Unknown {
                    self.adjust_confidence(0.05);
                    Some(game_combined)
                } else {
                    None
                }
            };

            match resolved {
                Some(game) => self.game = Some(game),
                None => self.clear_detection(),
            }
        } else {
            self.clear_detection();
        }
    }

    fn use_active_steam_app(&mut self) -> Result<bool, WayclipError> {
        let proc_dir = fs::read_dir("/proc")?;
        let mut appid: Option<String> = None;

        for entry in proc_dir.flatten() {
            let file_name = entry.file_name();
            let file_name_str = file_name.to_string_lossy();

            if file_name_str.chars().all(|c| c.is_ascii_digit()) {
                let environ_path = entry.path().join("environ");

                if let Ok(env_data) = fs::read(environ_path) {
                    let mut current_var = Vec::new();
                    for &byte in &env_data {
                        if byte == 0 {
                            let var = String::from_utf8_lossy(&current_var);
                            if var.starts_with("SteamAppId=")
                                && let Some(id) = var.split('=').nth(1)
                            {
                                appid = Some(id.to_string());
                            }
                            current_var.clear();
                        } else {
                            current_var.push(byte)
                        }
                    }
                }

                if appid.is_some() {
                    break;
                }
            }
        }

        let app_id = match appid {
            Some(id) => id,
            None => {
                log::debug!("No steam game running");
                return Ok(false);
            }
        };

        if let Ok(numeric_app_id) = app_id.parse::<u64>() {
            let by_appid = ClipsGames::from_steam_appid(numeric_app_id);
            if by_appid != ClipsGames::Unknown {
                log::debug!("Currently playing (Steam AppID {numeric_app_id}): {by_appid}");
                self.game = Some(by_appid);
                self.confidence = APPID_CONFIDENCE;
                return Ok(true);
            }
        }

        let home_path =
            dirs::home_dir().ok_or_else(|| WayclipError::NotFound("No home dir found".into()))?;
        let home = home_path.to_string_lossy();
        let search_dirs = vec![
            PathBuf::from(format!("{}/.steam", home)),
            PathBuf::from(format!("{}/.local/share/Steam", home)),
            PathBuf::from(format!("{}/.var/app/com.valvesoftware.Steam", home)),
            PathBuf::from(format!("{}/snap/steam", home)),
        ];

        let lib_vdf = Discovery::find_file_in_dirs(&search_dirs, "libraryfolders.vdf", 4);

        let Some(vdf_path) = lib_vdf else {
            log::warn!("Playing App ID: {} (libraryfolders.vdf not found)", app_id);
            return Ok(false);
        };

        let drives = Discovery::get_steam_drives(&vdf_path);
        for drive in drives {
            let manifest_path = PathBuf::from(&drive)
                .join("steamapps")
                .join(format!("appmanifest_{}.acf", app_id));

            if manifest_path.exists()
                && let Some(game_name) = Discovery::get_game_name(&manifest_path)
            {
                let detected = ClipsGames::from_title(&game_name);

                if detected == ClipsGames::Unknown {
                    log::debug!(
                        "Steam game '{}' (AppID {}) not found in games database",
                        game_name,
                        app_id
                    );
                    return Ok(false);
                }

                log::debug!("Currently playing: {} ({})", game_name, detected);
                self.game = Some(detected);
                self.confidence = MANIFEST_NAME_CONFIDENCE;
                return Ok(true);
            }
        }

        log::warn!(
            "Playing App ID: {} (Manifest not found on known drives)",
            app_id
        );

        Ok(false)
    }

    pub fn find_file_in_dirs(
        base_dirs: &[PathBuf],
        target_file: &str,
        max_depth: usize,
    ) -> Option<PathBuf> {
        fn search(
            dir: &Path,
            target: &str,
            current_depth: usize,
            max_depth: usize,
        ) -> Option<PathBuf> {
            if current_depth > max_depth {
                return None;
            }

            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();

                    if path.is_symlink() {
                        continue;
                    }

                    if path.is_dir() {
                        if let Some(found) = search(&path, target, current_depth + 1, max_depth) {
                            return Some(found);
                        }
                    } else if path.is_file() && path.file_name().unwrap_or_default() == target {
                        return Some(path);
                    }
                }
            }
            None
        }

        for base in base_dirs {
            if base.exists()
                && let Some(found) = search(base, target_file, 1, max_depth)
            {
                return Some(found);
            }
        }
        None
    }

    pub fn get_steam_drives(vdf_path: &Path) -> Vec<String> {
        let mut drives = Vec::new();
        if let Ok(file) = File::open(vdf_path) {
            let reader = BufReader::new(file);
            for line in reader.lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if trimmed.starts_with("\"path\"") {
                    let parts: Vec<&str> = trimmed.split('"').collect();
                    if parts.len() >= 4 {
                        drives.push(parts[3].to_string());
                    }
                }
            }
        }
        drives
    }

    pub fn get_game_name(manifest_path: &Path) -> Option<String> {
        if let Ok(file) = File::open(manifest_path) {
            let reader = BufReader::new(file);
            for line in reader.lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if trimmed.starts_with("\"name\"") {
                    let parts: Vec<&str> = trimmed.split('"').collect();
                    if parts.len() >= 4 {
                        return Some(parts[3].to_string());
                    }
                }
            }
        }
        None
    }

    fn use_active_window(&mut self) -> Result<(), WayclipError> {
        self.active_window_title = None;
        self.active_window_process_name = None;

        let mut confidence = 0.6;

        match self.desktop_env {
            DesktopEnvironmentType::Hyprland => {
                let res = Command::new("hyprctl")
                    .arg("activewindow")
                    .arg("-j")
                    .output()?;

                if !res.status.success() {
                    return Err(WayclipError::Discovery(
                        "hyprctl activewindow failed".into(),
                    ));
                }

                let json: HyprctlActiveWindow = serde_json::from_slice(&res.stdout)?;

                if json.fullscreen == 1 {
                    confidence += 0.2;
                }

                self.active_window_title = Some(json.title.clone());
                self.active_window_process_name = Some(json.class.clone());
                self.confidence = confidence;
            }
            DesktopEnvironmentType::Sway => {
                let res = Command::new("swaymsg").arg("-t").arg("get_tree").output()?;

                if !res.status.success() {
                    return Err(WayclipError::Discovery("swaymsg get_tree failed".into()));
                }

                let root: SwayNode = serde_json::from_slice(&res.stdout)?;

                fn find_sway_focused(node: &SwayNode) -> Option<&SwayNode> {
                    if node.focused == Some(true) {
                        return Some(node);
                    }
                    node.nodes
                        .iter()
                        .flatten()
                        .find_map(find_sway_focused)
                        .or_else(|| {
                            node.floating_nodes
                                .iter()
                                .flatten()
                                .find_map(find_sway_focused)
                        })
                }

                if let Some(focused) = find_sway_focused(&root) {
                    confidence += (focused.fullscreen_mode.unwrap_or(0) != 0) as u8 as f32 * 0.2;
                    self.active_window_title = focused.name.clone();
                    self.active_window_process_name = focused.app_id.clone().or_else(|| {
                        focused.pid.and_then(|pid| {
                            std::fs::read_to_string(format!("/proc/{pid}/comm"))
                                .ok()
                                .map(|s| s.trim().to_string())
                        })
                    });
                    self.confidence = confidence;
                } else {
                    return Err(WayclipError::Discovery("no focused sway node found".into()));
                }
            }
            DesktopEnvironmentType::Gnome => {
                let title_out = Command::new("gdbus")
                    .arg("call")
                    .arg("--session")
                    .arg("--dest")
                    .arg("org.gnome.Shell")
                    .arg("--object-path")
                    .arg("/org/gnome/Shell/Extensions/WindowsExt")
                    .arg("--method")
                    .arg("org.gnome.Shell.Extensions.WindowsExt.FocusTitle")
                    .output();

                if let Ok(out) = title_out
                    && out.status.success()
                    && let Ok(s) = str::from_utf8(&out.stdout)
                {
                    let parsed = s
                        .trim()
                        .trim_matches('(')
                        .trim_matches(')')
                        .trim()
                        .trim_matches('\'')
                        .to_string();

                    if !parsed.is_empty() {
                        self.active_window_title = Some(parsed);
                    }
                }

                let pid_out = Command::new("gdbus")
                    .arg("call")
                    .arg("--session")
                    .arg("--dest")
                    .arg("org.gnome.Shell")
                    .arg("--object-path")
                    .arg("/org/gnome/Shell/Extensions/WindowsExt")
                    .arg("--method")
                    .arg("org.gnome.Shell.Extensions.WindowsExt.FocusPID")
                    .output();

                if let Ok(out) = pid_out
                    && out.status.success()
                    && let Ok(s) = str::from_utf8(&out.stdout)
                {
                    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();

                    if let Ok(pid) = digits.parse::<u32>()
                        && let Ok(comm) = std::fs::read_to_string(format!("/proc/{}/comm", pid))
                    {
                        self.active_window_process_name = Some(comm.trim().to_string());
                    }
                }

                if self.active_window_title.is_none() && self.active_window_process_name.is_none() {
                    return Err(WayclipError::Discovery(
                        "gnome window detection produced no data (is the WindowsExt shell extension installed?)".into()
                    ));
                }

                self.confidence = confidence;
            }
            _ if self.session_type == SessionType::X11 => {
                let id_out = Command::new("xdotool").arg("getwindowfocus").output();

                if let Ok(out) = id_out
                    && out.status.success()
                    && let Ok(s) = str::from_utf8(&out.stdout)
                {
                    let winid = s.trim();

                    let title_out = Command::new("xdotool")
                        .arg("getwindowname")
                        .arg(winid)
                        .output();

                    if let Ok(tout) = title_out
                        && tout.status.success()
                        && let Ok(ts) = str::from_utf8(&tout.stdout)
                    {
                        self.active_window_title = Some(ts.trim().to_string());
                    }

                    let pid_out = Command::new("xprop")
                        .arg("-id")
                        .arg(winid)
                        .arg("_NET_WM_PID")
                        .output();

                    if let Ok(pop) = pid_out
                        && pop.status.success()
                        && let Ok(ps) = str::from_utf8(&pop.stdout)
                    {
                        let digits: String = ps.chars().filter(|c| c.is_ascii_digit()).collect();

                        if let Ok(pid) = digits.parse::<u32>()
                            && let Ok(comm) = std::fs::read_to_string(format!("/proc/{}/comm", pid))
                        {
                            self.active_window_process_name = Some(comm.trim().to_string());
                        }
                    }

                    let state_out = Command::new("xprop")
                        .arg("-id")
                        .arg(winid)
                        .arg("_NET_WM_STATE")
                        .output();

                    if let Ok(sout) = state_out
                        && sout.status.success()
                        && let Ok(ss) = str::from_utf8(&sout.stdout)
                        && ss.contains("_NET_WM_STATE_FULLSCREEN")
                    {
                        confidence += 0.2;
                    }

                    self.confidence = confidence;
                } else {
                    return Err(WayclipError::Discovery(
                        "xdotool getwindowfocus failed".into(),
                    ));
                }
            }
            _ => {
                return Err(WayclipError::Discovery(
                    format!(
                        "unsupported desktop environment: {:?} ({:?})",
                        self.desktop_env, self.session_type
                    )
                    .into(),
                ));
            }
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HyprctlActiveWindow {
    pub fullscreen: u32,
    pub title: String,
    pub class: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwayNode {
    pub name: Option<String>,
    pub app_id: Option<String>,
    pub pid: Option<u32>,
    pub fullscreen_mode: Option<i32>,
    pub focused: Option<bool>,
    pub nodes: Option<Vec<SwayNode>>,
    pub floating_nodes: Option<Vec<SwayNode>>,
}
