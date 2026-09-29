use crate::linux::core::ipc::commands::IpcCommand;
use crate::linux::manager::DaemonManager;
use ksni::MenuItem;
use ksni::TrayMethods;
use ksni::menu::StandardItem;
use sysinfo::Pid;
use sysinfo::System;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use wayclip_core::models::error::WayclipError;
use wayclip_core::settings::tray::TraySettings;

/// Make this logo compile-time dependant
static TRAY_LOGO_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/branding/pngs/wayclip-256x256.png"
));

// courrently this is only for linux, didnt find a tray lib for windows, but i bet its gonna be rly
// different anyway

#[derive(Clone)]
pub struct TrayStats {
    pub status: String,
    pub cpu: String,
    pub ram: String,
    // not really possible since depends highly on gpu
    // vram: String,
    // gpu: String,
}

#[derive(Clone)]
pub struct WayclipTray {
    command_sender: mpsc::Sender<IpcCommand>,
    // we will update this using our handler
    pub stats: Option<TrayStats>,
    pub config: TraySettings,
    pub cancel_token: CancellationToken,
}

impl WayclipTray {
    pub fn run_tray(
        command_sender: mpsc::Sender<IpcCommand>,
        config: TraySettings,
        cancel_token: CancellationToken,
    ) {
        if !config.enabled {
            return;
        }

        tokio::spawn(async move {
            let cmd_sender = command_sender.clone();

            // create tray
            let tray = Self {
                command_sender,
                stats: None,
                config,
                cancel_token: cancel_token.clone(),
            };
            let poll = tray.config.show_stats || tray.config.show_status;

            // spawn handler, so we can also then edit it on the fly
            let handle = match tray.spawn().await {
                Ok(handle) => handle,
                Err(e) => {
                    log::error!("Failed to register tray: {e}");
                    return;
                }
            };

            log::info!("Tray registered successfully");

            if poll {
                // collect system info
                let mut sys = System::new_all();
                let pid = Pid::from(std::process::id() as usize);

                loop {
                    tokio::select! {
                         _ = cancel_token.cancelled() => {
                             log::debug!("Tray polling loop shutting down");
                             break;
                         }
                         _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                             // inf loop, update info on a specific pid
                             sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);

                             let (status, cpu, ram) = {
                                 let (tx, rx) = oneshot::channel();
                                 let status = if cmd_sender.send(IpcCommand::GetStatus { responder: tx }).await.is_ok() {
                                     match rx.await {
                                         Ok(st) => format!("{st:?}"),
                                         Err(_) => "Unknown".into(),
                                     }
                                 } else {
                                     "Offline".into()
                                 };

                                 // get cpu & mem for a process
                                 let mut cpu = 0.0;
                                 let mut mem = 0.0;
                                 if let Some(proc) = sys.process(pid) {
                                     // yes, its between 0-100. meaning across all cores
                                     cpu = proc.cpu_usage() / sys.cpus().len() as f32;
                                     // 1000, not 1024 since MB not MiB
                                     mem = proc.memory() as f64 / 1000.0 / 1000.0;
                                 }

                                 (status, format!("{:.1}%", cpu), format!("{:.1} MB", mem))
                             };

                             // and then use that data to actually update tray info
                             handle
                                 .update(move |t: &mut WayclipTray| {
                                     let stats = TrayStats { status, cpu, ram };
                                     t.stats = Some(stats);
                                 })
                                 .await;
                         }
                    }
                }
            } else {
                cancel_token.cancelled().await;
            }
        });
    }

    pub fn get_png(&self) -> Vec<u8> {
        TRAY_LOGO_PNG.to_vec()
    }
}

impl ksni::Tray for WayclipTray {
    fn id(&self) -> String {
        // hardcoded
        "org.wayclip.Tray".into()
    }

    fn icon_name(&self) -> String {
        String::from("wayclip")
    }

    fn title(&self) -> String {
        String::from("Wayclip")
    }

    // all of our menu actions
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let cmd_sender_for_save = self.command_sender.clone();

        vec![
            // main title
            StandardItem {
                label: "Wayclip".into(),
                icon_data: self.get_png(),
                enabled: false,
                visible: self.config.show_logo,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            // standart actions
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
                        if let Ok(mgr) = DaemonManager::new().await {
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
            // all of statistics
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
                    // could go for the combined look, but turns out too fat
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
