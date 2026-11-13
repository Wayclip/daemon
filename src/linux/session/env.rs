use std::{
    env,
    os::unix::net::UnixStream,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

const VARS: &[&str] = &[
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_DESKTOP",
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_RUNTIME_DIR",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "SWAYSOCK",
];

pub struct SessionVariable;

impl SessionVariable {
    // we gotta wait until every varaible is ready for us
    pub async fn wait_for_session(timeout: Duration) -> bool {
        let start = Instant::now();
        let mut backoff = Duration::from_millis(200);

        while start.elapsed() < timeout {
            Self::import_from_user_manager();

            if Self::is_display_server_ready() {
                return true;
            }

            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(2));
        }

        log::warn!("Timed out waiting for graphical session display socket");
        false
    }

    pub fn is_display_server_ready() -> bool {
        let has_desktop = env::var("XDG_CURRENT_DESKTOP")
            .map(|s| !s.is_empty())
            .unwrap_or(false);

        if !has_desktop {
            return false;
        }

        if let (Ok(runtime_dir), Ok(wayland_display)) =
            (env::var("XDG_RUNTIME_DIR"), env::var("WAYLAND_DISPLAY"))
            && !wayland_display.is_empty()
            && UnixStream::connect(PathBuf::from(runtime_dir).join(wayland_display)).is_ok()
        {
            return true;
        }

        if let Ok(display) = env::var("DISPLAY")
            && !display.is_empty()
        {
            let display_num = display
                .trim_start_matches(':')
                .split('.')
                .next()
                .unwrap_or("0");
            let socket_path = format!("/tmp/.X11-unix/X{display_num}");
            if UnixStream::connect(socket_path).is_ok() {
                return true;
            }
        }

        false
    }

    pub fn import_from_user_manager() {
        let missing: Vec<&str> = VARS
            .iter()
            .copied()
            .filter(|k| env::var(k).map(|v| v.is_empty()).unwrap_or(true))
            .collect();

        if missing.is_empty() {
            return;
        }

        let Ok(output) = Command::new("systemctl")
            .args(["--user", "show-environment"])
            .output()
        else {
            return;
        };

        if !output.status.success() {
            return;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut imported: Vec<&str> = Vec::new();

        for line in stdout.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };

            if missing.contains(&key) && !value.is_empty() {
                unsafe { env::set_var(key, value) };
                imported.push(key);
            }
        }

        if !imported.is_empty() {
            log::info!(
                "Imported session variables from user manager: {}",
                imported.join(", ")
            );
        }
    }
}
