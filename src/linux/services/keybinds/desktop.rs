use crate::linux::ipc::commands::IpcCommand;
use crate::linux::ipc::{
    DEFAULT_DBUS_INTERFACE as IFACE, DEFAULT_DBUS_SERVICE as SVC, DEFAULT_INTERFACE_PATH as PATH,
};
use log::info;
use std::{env, process::Command};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use wayclip_core::models::error::WayclipError;
use wayclip_core::models::input::keyboard::WayclipKeyCombo;
use wayclip_global_hotkey::GlobalHotKeyEvent;
use wayclip_global_hotkey::HotKeyState;
use wayclip_global_hotkey::{GlobalHotKeyManager, hotkey::HotKey};

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

pub struct DesktopManager {
    pub desktop: DesktopEnvironmentType,
    pub trigger_combo: WayclipKeyCombo,
    hotkey_manager: Option<GlobalHotKeyManager>,
    registered_hotkey: Option<HotKey>,
}

impl DesktopManager {
    pub fn new(trigger_combo: WayclipKeyCombo) -> Result<Self, WayclipError> {
        let (desktop, _) = Self::get_env_session()?;

        Ok(Self {
            desktop,
            trigger_combo,
            hotkey_manager: None,
            registered_hotkey: None,
        })
    }

    pub fn get_env_session() -> Result<(DesktopEnvironmentType, SessionType), WayclipError> {
        let session_env = env::var("XDG_SESSION_TYPE").unwrap_or("wayland".to_string());
        let session = match session_env.to_lowercase().as_str() {
            "wayland" => SessionType::Wayland,
            "x11" | "xorg" => SessionType::X11,
            _ => SessionType::Unknown,
        };

        let desktop_env = env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_lowercase();
        let desktop = desktop_env
            .split(':')
            .find_map(|d| match d {
                "hyprland" => Some(DesktopEnvironmentType::Hyprland),
                "gnome" => Some(DesktopEnvironmentType::Gnome),
                "sway" => Some(DesktopEnvironmentType::Sway),
                "kde" => Some(DesktopEnvironmentType::Kde),
                _ => None,
            })
            .unwrap_or(DesktopEnvironmentType::Unknown);

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

    // instead of calling CLI to execute a call, we just call dbus directly..
    fn get_trigger_command_string(&self) -> String {
        match self.desktop {
            DesktopEnvironmentType::Kde => format!("qdbus {SVC} {PATH} {IFACE}.SaveClip"),
            DesktopEnvironmentType::Gnome => format!(
                "gdbus call --session --dest {SVC} --object-path {PATH} --method {IFACE}.SaveClip"
            ),
            _ => format!("busctl --user call {SVC} {PATH} {IFACE} SaveClip"),
        }
    }

    fn remove_global_hotkey(&mut self) -> Result<(), WayclipError> {
        if let (Some(manager), Some(hotkey)) =
            (self.hotkey_manager.take(), self.registered_hotkey.take())
            && let Err(e) = manager.unregister(hotkey)
        {
            log::error!("Failed to unregister hotkey: {e:?}");
        }

        Ok(())
    }

    fn setup_global_hotkey(
        &mut self,
        command_sender: &mpsc::Sender<IpcCommand>,
    ) -> Result<(), WayclipError> {
        log::info!("Using wayclip_global_hotkey");

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

        let command_sender = command_sender.clone();
        let hotkey_id = hotkey.id();

        let handle = tokio::runtime::Handle::current();
        std::thread::Builder::new()
            .name("hotkey-listener".into())
            .spawn(move || {
                while let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
                    if event.id() == hotkey_id && event.state() == HotKeyState::Released {
                        let tx = command_sender.clone();
                        handle.spawn(async move {
                            let (sender, receiver) = oneshot::channel();
                            if tx
                                .send(IpcCommand::SaveClip {
                                    custom_name: None,
                                    responder: sender,
                                })
                                .await
                                .is_err()
                            {
                                log::warn!("Daemon core offline, could not trigger clip");
                                return;
                            }
                            match receiver.await {
                                Ok(Ok(name)) => {
                                    log::info!("Clip saved successfully via hotkey: {name}")
                                }
                                Ok(Err(e)) => log::error!("Failed to save clip: {e:?}"),
                                Err(_) => log::warn!("Daemon core dropped reply channel"),
                            }
                        });
                    }
                }
            })?;

        Ok(())
    }

    pub fn create_auto_bind(
        &mut self,
        command_sender: &mpsc::Sender<IpcCommand>,
    ) -> Result<(), WayclipError> {
        match self.desktop {
            DesktopEnvironmentType::Hyprland => {
                let bind_string = self.trigger_combo.clone().to_string().replace("+", " + ");
                let trigger_cmd = self.get_trigger_command_string();

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
                let trigger_cmd = self.get_trigger_command_string();

                let mut cmd = Command::new("swaymsg");
                cmd.arg("bindsym")
                    .arg(&bind_string)
                    .arg("exec")
                    .arg(&trigger_cmd);
                Self::run_command(cmd, "swaymsg bindsym")?;
            }
            //DesktopEnvironmentType::Hyprland
            DesktopEnvironmentType::Gnome | DesktopEnvironmentType::Kde => {
                self.setup_global_hotkey(command_sender)?
            }
            _ => info!(
                "No auto bind setup available for your desktop environment. Please bind {} to {}",
                self.trigger_combo,
                self.get_trigger_command_string()
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
