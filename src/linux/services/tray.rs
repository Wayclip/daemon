use ksni::MenuItem;
use ksni::TrayMethods;
use ksni::menu::StandardItem;
use std::time::Duration;
use sysinfo::Pid;
use sysinfo::System;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use wayclip_core::models::daemon::client::DaemonClient;
use wayclip_core::settings::tray::TraySettings;

use crate::linux::ipc::commands::IpcCommand;

static TRAY_LOGO_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/branding/pngs/wayclip-256x256.png"
));

#[derive(Clone)]
pub struct TrayStats {
    pub status: String,
    pub cpu: String,
    pub ram: String,
}

#[derive(Clone)]
struct Tray {
    command_sender: mpsc::Sender<IpcCommand>,
    stats: Option<TrayStats>,
    config: TraySettings,
    cancel_token: CancellationToken,
}

impl Tray {
    fn get_png(&self) -> Vec<u8> {
        TRAY_LOGO_PNG.to_vec()
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "org.wayclip.Tray".into()
    }

    // TODO: ICON HAS TO BE PRESENT IN  /usr/share/icons/hicolor/scalable/apps/...
    fn icon_name(&self) -> String {
        String::from("wayclip")
    }

    fn title(&self) -> String {
        String::from("Wayclip")
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let cmd_sender_for_save = self.command_sender.clone();

        vec![
            StandardItem {
                label: "Wayclip".into(),
                icon_data: self.get_png(),
                enabled: false,
                visible: self.config.show_logo,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Save Clip".into(),
                activate: Box::new(move |_| {
                    let sender = cmd_sender_for_save.clone();
                    tokio::spawn(async move {
                        let (tx, rx) = oneshot::channel();
                        if let Err(e) = sender
                            .send(IpcCommand::SaveClip {
                                custom_name: None,
                                responder: tx,
                            })
                            .await
                        {
                            log::error!("Tray failed to trigger save: {e}");
                            return;
                        }

                        if let Ok(Err(e)) = rx.await {
                            log::error!("Tray failed to trigger save: {e}");
                        }
                    });
                }),
                visible: self.config.show_save_clip,
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Restart Daemon".into(),
                activate: Box::new(|_| {
                    tokio::spawn(async move {
                        if let Ok(mgr) = DaemonClient::new().await {
                            let _ = mgr.restart_daemon().await;
                        }
                    });
                }),
                visible: self.config.show_restart,
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Exit Wayclip".into(),
                activate: Box::new(|this: &mut Self| {
                    this.cancel_token.cancel();
                }),
                visible: self.config.show_exit,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: format!(
                    "Status: {}",
                    self.stats
                        .as_ref()
                        .map(|s| s.status.clone())
                        .unwrap_or_default()
                ),
                visible: self.stats.is_some() && self.config.show_status,
                enabled: false,
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: format!(
                    "CPU: {} • RAM: {}",
                    self.stats
                        .as_ref()
                        .map(|s| s.cpu.clone())
                        .unwrap_or_default(),
                    self.stats
                        .as_ref()
                        .map(|s| s.ram.clone())
                        .unwrap_or_default(),
                ),
                visible: self.stats.is_some() && self.config.show_stats,
                enabled: false,
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub struct TrayManager {
    config: TraySettings,
    cancel_token: CancellationToken,
    task_handle: Option<JoinHandle<()>>,
}

impl TrayManager {
    pub fn new(config: TraySettings, cancel_token: CancellationToken) -> Self {
        Self {
            config,
            cancel_token,
            task_handle: None,
        }
    }

    pub fn start(&mut self, command_sender: &mpsc::Sender<IpcCommand>) {
        if !self.config.enabled || self.task_handle.is_some() {
            return;
        }

        let cancel_token = self.cancel_token.clone();

        let cmd_sender = command_sender.clone();
        let config = self.config.clone();

        let handle = tokio::spawn(async move {
            let tray = Tray {
                command_sender: cmd_sender.clone(),
                stats: None,
                config: config.clone(),
                cancel_token: cancel_token.clone(),
            };
            let poll = config.show_stats || config.show_status;

            let handle = match tray.spawn().await {
                Ok(handle) => handle,
                Err(e) => {
                    log::error!("Failed to register tray: {e}");
                    return;
                }
            };

            log::info!("Tray registered successfully");

            if poll {
                let mut sys = System::new_all();
                let pid = Pid::from(std::process::id() as usize);

                loop {
                    tokio::select! {
                        _ = cancel_token.cancelled() => {
                            log::debug!("Tray polling loop shutting down");
                            break;
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                            sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);

                            let (status, cpu, ram) = {
                                let (tx, rx) = oneshot::channel();
                                let status = if cmd_sender.send(IpcCommand::GetStatus { responder: tx }).await.is_ok() {
                                    match tokio::time::timeout(Duration::from_secs(1), rx).await {
                                        Ok(Ok(st)) => format!("{st:?}"),
                                        _ => "Unknown".into(),
                                    }
                                } else { "Offline".into() };

                                let mut cpu = 0.0;
                                let mut mem = 0.0;
                                if let Some(proc) = sys.process(pid) {
                                    cpu = proc.cpu_usage() / sys.cpus().len() as f32;
                                    mem = proc.memory() as f64 / 1000.0 / 1000.0;
                                }

                                (status, format!("{:.1}%", cpu), format!("{:.1} MB", mem))
                            };

                            handle
                                .update(move |t: &mut Tray| {
                                    t.stats = Some(TrayStats { status, cpu, ram });
                                })
                                .await;
                        }
                    }
                }
            } else {
                cancel_token.cancelled().await;
            }
        });

        self.task_handle = Some(handle);
    }

    pub async fn stop(&mut self) {
        self.cancel_token.cancel();
        if let Some(mut handle) = self.task_handle.take() {
            if tokio::time::timeout(Duration::from_secs(2), &mut handle)
                .await
                .is_err()
            {
                handle.abort();
            }
        }
    }
}
