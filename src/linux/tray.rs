use crate::linux::core::DaemonCore;
use crate::linux::core::types::DaemonStatus;
use crate::linux::manager::DaemonManager;
use ksni::MenuItem;
use ksni::TrayMethods;
use ksni::menu::StandardItem;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use sysinfo::Pid;
use sysinfo::System;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use wayclip_core::models::error::WayclipError;
use wayclip_core::settings::tray::TraySettings;

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
    pub daemon: Arc<Mutex<DaemonCore>>,
    // we will update this using our handler
    pub stats: Option<TrayStats>,
    pub config: TraySettings,
    pub cancel_token: CancellationToken,
}

impl WayclipTray {
    pub async fn run_tray(
        daemon: Arc<Mutex<DaemonCore>>,
        config: TraySettings,
        cancel_token: CancellationToken,
    ) -> Result<(), WayclipError> {
        if !config.enabled {
            return Ok(());
        }

        // create tray
        let tray = Self {
            daemon: daemon.clone(),
            stats: None,
            config,
            cancel_token: cancel_token.clone(),
        };
        let poll = tray.config.show_stats || tray.config.show_status;

        // spawn handler, so we can also then edit it on the fly
        let handle = tray
            .spawn()
            .await
            .map_err(|e| WayclipError::Tray(e.to_string().into()))?;

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
                             let core = daemon.lock().await;
                             // get status
                             let status = format!(
                                 "{:?}",
                                 core.get_status().await.unwrap_or(DaemonStatus::Inactive)
                             );

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

        Ok(())
    }

    pub fn get_png(&self) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap_or(&PathBuf::default())
            .join("branding")
            .join("pngs")
            .join("wayclip-256x256-p8.png");

        fs::read(path).unwrap_or_default()
    }
}

impl ksni::Tray for WayclipTray {
    fn id(&self) -> String {
        // hardcoded
        "org.wayclip.Tray".into()
    }

    fn icon_name(&self) -> String {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap_or(&PathBuf::default())
            .join("branding")
            .join("svgs")
            .join("wayclip.svg")
            .to_string_lossy()
            .to_string()
    }

    fn title(&self) -> String {
        String::from("Wayclip")
    }

    // all of our menu actions
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let daemon_for_save = self.daemon.clone();

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
                    // yes i agree ugly ahh code, but whatever
                    let core = daemon_for_save.clone();
                    tokio::spawn(async move {
                        if let Err(e) = DaemonCore::save_clip(core, None).await {
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
                label: "Exit Tray & Daemon".into(),
                activate: Box::new(|this: &mut Self| {
                    let token = this.cancel_token.clone();
                    tokio::spawn(async move {
                        let stopped_via_systemd = match DaemonManager::new().await {
                            Ok(mgr) => mgr.stop_daemon().await.is_ok(),
                            Err(_) => false,
                        };
                        if !stopped_via_systemd {
                            token.cancel();
                        }
                    });
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
            // StandardItem {
            //     label: format!(
            //         "RAM: {}",
            //         self.stats
            //             .as_ref()
            //             .map(|s| s.ram.clone())
            //             .unwrap_or_default()
            //     ),
            //     visible: self.stats.is_some(),
            //     enabled: false,
            //     ..Default::default()
            // }
            // .into(),
        ]
    }
}
