use crate::linux::ipc::commands::IpcCommand;
use log::info;
use std::thread::JoinHandle;
use std::{env, process::Command, time::Duration};
use tokio::{runtime::Handle, sync::mpsc, sync::oneshot};
use tokio_util::sync::CancellationToken;
use wayclip_core::models::daemon::{
    DEFAULT_DBUS_INTERFACE as IFACE, DEFAULT_DBUS_SERVICE as SVC, DEFAULT_INTERFACE_PATH as PATH,
};
use wayclip_core::models::error::WayclipError;
use wayclip_core::models::input::keyboard::WayclipKeyCombo;
use wayclip_global_hotkey::GlobalHotKeyEvent;
use wayclip_global_hotkey::HotKeyState;
use wayclip_global_hotkey::{GlobalHotKeyManager, hotkey::HotKey};

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
    listener: Option<JoinHandle<()>>,
    cancel_token: CancellationToken,
}

impl DesktopManager {
    pub fn new(
        trigger_combo: WayclipKeyCombo,
        cancel_token: CancellationToken,
    ) -> Result<Self, WayclipError> {
        Ok(Self {
            desktop: Self::detect_desktop(),
            trigger_combo,
            hotkey_manager: None,
            registered_hotkey: None,
            listener: None,
            cancel_token,
        })
    }

    pub fn detect_desktop() -> DesktopEnvironmentType {
        env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_lowercase()
            .split(':')
            .find_map(|d| {
                let d = d.trim();
                if d.contains("hyprland") {
                    Some(DesktopEnvironmentType::Hyprland)
                } else if d.contains("gnome") {
                    Some(DesktopEnvironmentType::Gnome)
                } else if d.contains("sway") {
                    Some(DesktopEnvironmentType::Sway)
                } else if d.contains("kde") || d.contains("plasma") {
                    Some(DesktopEnvironmentType::Kde)
                } else {
                    None
                }
            })
            .unwrap_or(DesktopEnvironmentType::Unknown)
    }

    fn run_command(mut cmd: Command, desc: &str) -> Result<(), WayclipError> {
        let output = cmd
            .output()
            .map_err(|e| WayclipError::Error(format!("Failed to execute '{desc}': {e}").into()))?;

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

            return Err(WayclipError::Error(
                format!("Command '{desc}' failed: {err_msg}").into(),
            ));
        }

        Ok(())
    }

    fn escape(value: &str) -> String {
        value.replace('\\', "\\\\").replace('"', "\\\"")
    }

    // instead of calling CLI to execute a call, we just call dbus directly..
    fn get_trigger_command_string(&self) -> String {
        match self.desktop {
            DesktopEnvironmentType::Kde
            | DesktopEnvironmentType::Gnome
            | DesktopEnvironmentType::Unknown => format!(
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

        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }

        Ok(())
    }

    fn setup_global_hotkey(
        &mut self,
        command_sender: &mpsc::Sender<IpcCommand>,
        handle: Handle,
    ) -> Result<(), WayclipError> {
        let manager = GlobalHotKeyManager::new().map_err(|e| {
            WayclipError::Error(format!("Failed to initialize GlobalHotKeyManager: {e:?}").into())
        })?;

        let hotkey = HotKey::new(
            Some(self.trigger_combo.key_modifiers.clone().into()),
            self.trigger_combo.key_code.clone().into(),
        );

        manager
            .register(hotkey)
            .map_err(|e| WayclipError::Error(format!("Failed to register hotkey: {e:?}").into()))?;

        self.hotkey_manager = Some(manager);
        self.registered_hotkey = Some(hotkey);

        let command_sender = command_sender.clone();
        let hotkey_id = hotkey.id();
        let cancel_token = self.cancel_token.clone();

        let listener = std::thread::Builder::new()
            .name("hotkey-listener".into())
            .spawn(move || {
                while !cancel_token.is_cancelled() {
                    let Ok(event) =
                        GlobalHotKeyEvent::receiver().recv_timeout(Duration::from_millis(200))
                    else {
                        continue;
                    };

                    if event.id() != hotkey_id || event.state() != HotKeyState::Released {
                        continue;
                    }

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
                            log::warn!("could not trigger clip");
                            return;
                        }
                        match receiver.await {
                            Ok(Ok(name)) => {
                                log::info!("clip saved successfully via hotkey {name}")
                            }
                            Ok(Err(e)) => log::error!("failed to save clip {e:?}"),
                            Err(_) => log::warn!("dropped reply channel"),
                        }
                    });
                }
            })?;

        log::debug!("Registered a shortcut");
        self.listener = Some(listener);

        Ok(())
    }

    pub fn create_auto_bind(
        &mut self,
        command_sender: &mpsc::Sender<IpcCommand>,
        handle: Handle,
    ) -> Result<(), WayclipError> {
        self.desktop = Self::detect_desktop();

        match self.desktop {
            DesktopEnvironmentType::Hyprland => {
                let bind_string =
                    Self::escape(&self.trigger_combo.clone().to_string().replace("+", " + "));
                let trigger_cmd = Self::escape(&self.get_trigger_command_string());

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
            _ => self.setup_global_hotkey(command_sender, handle)?,
        }

        Ok(())
    }

    pub fn remove_auto_bind(&mut self) -> Result<(), WayclipError> {
        match self.desktop {
            DesktopEnvironmentType::Hyprland => {
                let bind_string =
                    Self::escape(&self.trigger_combo.clone().to_string().replace("+", " + "));
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
            _ => info!("No auto bind removal available for your desktop environment"),
        }
        self.remove_global_hotkey()
    }
}
