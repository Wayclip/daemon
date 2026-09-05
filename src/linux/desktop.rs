use crate::linux::core::DaemonCore;
use log::error;
use log::info;
use std::{env, process::Command, sync::Arc};
use tokio::sync::Mutex;
use wayclip_core::models::error::WayclipError;
use wayclip_core::models::input::keyboard::WayclipKeyCombo;
use wayclip_global_hotkey::GlobalHotKeyEvent;
use wayclip_global_hotkey::HotKeyState;
use wayclip_global_hotkey::{GlobalHotKeyManager, hotkey::HotKey};

// This is specifically only for linux.
// handles different distros and environments to make a bind

// i will be real, there is no *real* use for this yet... since im pretty sure everything here so
// far works both wayland & x11
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SessionType {
    X11,
    Wayland,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DesktopEnvironmentType {
    Hyprland,
    Gnome,
    Sway,
    Kde,
    #[default]
    Unknown,
}

pub struct DesktopEnvironmentManager {
    pub daemon: Arc<Mutex<DaemonCore>>,
    pub desktop: DesktopEnvironmentType,
    pub trigger_combo: WayclipKeyCombo,
    hotkey_manager: Option<GlobalHotKeyManager>,
    registered_hotkey: Option<HotKey>,
}

impl DesktopEnvironmentManager {
    pub fn new(
        daemon: Arc<Mutex<DaemonCore>>,
        trigger_combo: WayclipKeyCombo,
    ) -> Result<Self, WayclipError> {
        let (desktop, _) = Self::get_env_session()?;

        Ok(Self {
            daemon,
            desktop,
            trigger_combo,
            hotkey_manager: None,
            registered_hotkey: None,
        })
    }

    pub fn get_env_session() -> Result<(DesktopEnvironmentType, SessionType), WayclipError> {
        // default to wayland since way-clip. way -> wayland. haha
        let session_env = env::var("XDG_SESSION_TYPE").unwrap_or("wayland".to_string());
        let session = match session_env.to_lowercase().as_str() {
            "wayland" => SessionType::Wayland,
            "x11" | "xorg" => SessionType::X11,
            _ => SessionType::Unknown,
        };

        // safer to assume gnome is being used
        let desktop_env = env::var("XDG_CURRENT_DESKTOP").unwrap_or("gnome".to_string());
        let desktop = match desktop_env.to_lowercase().as_str() {
            "hyprland" => DesktopEnvironmentType::Hyprland,
            "gnome" => DesktopEnvironmentType::Gnome,
            "sway" => DesktopEnvironmentType::Sway,
            "kde" => DesktopEnvironmentType::Kde,
            // assume simplest, instead of killing program
            _ => DesktopEnvironmentType::Gnome,
        };

        Ok((desktop, session))
    }

    fn run_command(mut cmd: Command, desc: &str) -> Result<(), WayclipError> {
        let output = cmd
            .output()
            .map_err(|e| WayclipError::CLI(format!("Failed to execute '{desc}': {e}").into()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

            let err_msg = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                format!("process exited with code {:?}", output.status.code())
            };

            return Err(WayclipError::CLI(
                format!("Command '{desc}' failed: {err_msg}").into(),
            ));
        }

        Ok(())
    }

    fn get_trigger_command_string(&self) -> Result<String, WayclipError> {
        let env = env::current_exe()?;
        let parent = env
            .parent()
            .ok_or_else(|| WayclipError::NotFound("No parent found".into()))?;

        // TODO: for now the CLI crate is named `wayclip-cli-linux`, hence the binary would also be called this way
        let process_path = parent.join("wayclip-cli-linux");
        let process_string = process_path
            .to_str()
            .ok_or_else(|| WayclipError::Validation("Could not convert to str".into()))?
            .to_string();

        Ok(format!("{process_string} daemon save"))
    }

    pub fn remove_global_hotkey(&mut self) -> Result<(), WayclipError> {
        if let (Some(manager), Some(hotkey)) =
            (self.hotkey_manager.take(), self.registered_hotkey.take())
            && let Err(e) = manager.unregister(hotkey)
        {
            log::error!("Failed to unregister hotkey: {e:?}");
        }

        Ok(())
    }

    pub fn setup_global_hotkey(&mut self) -> Result<(), WayclipError> {
        log::info!("Using wayclip_global_hotkey");

        let (desktop, session) = Self::get_env_session()?;
        if desktop == DesktopEnvironmentType::Gnome && session == SessionType::Wayland {
            log::info!(
                "GNOME Wayland detected, forcing GDK_BACKEND=x11 for global_hotkey fallback"
            );
            unsafe {
                env::set_var("GDK_BACKEND", "x11");
            }
        }

        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => m,
            Err(e) => {
                log::error!("Failed to initialize GlobalHotKeyManager (portal missing?): {e:?}");
                return Ok(());
            }
        };

        let hotkey = HotKey::new(
            Some(self.trigger_combo.key_modifiers.clone().into()),
            self.trigger_combo.key_code.clone().into(),
        );

        if let Err(e) = manager.register(hotkey) {
            log::error!("Failed to register global hotkey: {e:?}");
            return Ok(());
        }

        log::debug!("Registered a shortcut");
        self.hotkey_manager = Some(manager);
        self.registered_hotkey = Some(hotkey);

        let daemon_clone = self.daemon.clone();

        tokio::task::spawn_blocking(move || {
            // apparently to keep it alive and not drop it prematurely
            while let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
                if event.id() == hotkey.id() && event.state() == HotKeyState::Released {
                    log::debug!("Hotkey triggered");
                    let daemon_clone = Arc::clone(&daemon_clone);
                    tokio::spawn(async move {
                        if let Err(e) = DaemonCore::save_clip(daemon_clone, None).await {
                            error!("Error saving clip via shortcut: {e}")
                        }
                    });
                }
            }
        });

        Ok(())
    }

    pub fn create_auto_bind(&mut self) -> Result<(), WayclipError> {
        match self.desktop {
            DesktopEnvironmentType::Hyprland => {
                let bind_string = self.trigger_combo.clone().to_string().replace("+", " + ");
                let trigger_cmd = self.get_trigger_command_string()?;

                let full_string = format!(
                    "hl.bind(\"{}\", hl.dsp.exec_cmd(\"{}\"))",
                    bind_string, trigger_cmd
                );

                let mut cmd = Command::new("hyprctl");
                cmd.arg("eval").arg(&full_string);
                Self::run_command(cmd, "hyprctl eval")?;
            }
            DesktopEnvironmentType::Sway => {
                let bind_string = self.trigger_combo.to_string();
                let trigger_cmd = self.get_trigger_command_string()?;

                let mut cmd = Command::new("swaymsg");
                cmd.arg("bindsym")
                    .arg(&bind_string)
                    .arg("exec")
                    .arg(&trigger_cmd);
                Self::run_command(cmd, "swaymsg bindsym")?;
            }
            //DesktopEnvironmentType::Hyprland
            DesktopEnvironmentType::Gnome | DesktopEnvironmentType::Kde => {
                self.setup_global_hotkey()?
            }
            _ => info!(
                "No auto bind setup available for your desktop environment. Please bind {} to {}",
                self.trigger_combo,
                self.get_trigger_command_string()?
            ),
        }

        Ok(())
    }

    pub fn remove_auto_bind(&mut self) -> Result<(), WayclipError> {
        match self.desktop {
            DesktopEnvironmentType::Hyprland => {
                let bind_string = self.trigger_combo.clone().to_string().replace("+", " + ");
                let full_string = format!("hl.unbind(\"{}\")", bind_string);

                let mut cmd = Command::new("hyprctl");
                cmd.arg("eval").arg(&full_string);
                Self::run_command(cmd, "hyprctl eval unbind")?;
            }
            DesktopEnvironmentType::Sway => {
                let bind_string = self.trigger_combo.to_string();

                let mut cmd = Command::new("swaymsg");
                cmd.arg("unbindsym").arg(&bind_string);
                Self::run_command(cmd, "swaymsg unbindsym")?;
            }
            //DesktopEnvironmentType::Hyprland
            DesktopEnvironmentType::Gnome | DesktopEnvironmentType::Kde => {
                self.remove_global_hotkey()?
            }
            _ => info!("No auto bind removal available for your desktop environment",),
        }

        Ok(())
    }
}

// Muight not always run automatically, so we do some manual calls too
impl Drop for DesktopEnvironmentManager {
    fn drop(&mut self) {
        if let Err(e) = self.remove_auto_bind() {
            log::warn!("Failed to unbind hotkeys on drop: {e:?}");
        }
    }
}
