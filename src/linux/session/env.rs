use std::{env, process::Command};

const VARS: &[&str] = &[
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_DESKTOP",
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_RUNTIME_DIR",
];

pub struct SessionVariable;

impl SessionVariable {
    pub fn import_from_user_manager() {
        let missing: Vec<&str> = VARS
            .iter()
            .copied()
            .filter(|k| env::var(k).map(|v| v.is_empty()).unwrap_or(true))
            .collect();

        if missing.is_empty() {
            return;
        }

        // dont exactly like this, but easiest way
        let Ok(output) = Command::new("systemctl")
            .args(["--user", "show-environment"])
            .output()
        else {
            log::warn!("could not from systemctl",);
            return;
        };

        if !output.status.success() {
            log::warn!("systemctl failed");
            return;
        }

        let mut imported: Vec<&str> = Vec::new();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

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
                "recovered session variables from the user manager: {}",
                imported.join(", ")
            );
        } else {
            log::warn!("session is not up yet",);
        }
    }
}
