use crate::{
    common::notifications::{NotificationEvent, NotificationManager},
    linux::{core::DEFAULT_PIPEWIRE_TIMEOUT, pipewire::PipewireManager},
};
use ashpd::desktop::screencast::Screencast;
use serde::{Deserialize, Serialize};
use std::{
    env,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;
use wayclip_core::settings::{
    output::VideoFormat,
    recording::{AudioCodec, CodecType, VideoCodec},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoctorResult {
    pub name: String,
    pub ok: bool,
    pub required: bool,
    pub details: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Doctor {
    pub audio_codec: AudioCodec,
    pub video_codec: VideoCodec,
    pub video_format: VideoFormat,
    pub clip_directory: PathBuf,
    pub preview_directory: PathBuf,
    pub metadata_directory: PathBuf,
    pub microphone: (String, bool),
    pub background: (String, bool),
}

pub const XDG_PORTAL_TIMEOUT_S: u64 = 4;

impl Doctor {
    pub async fn run_all_checks(&self) -> Vec<DoctorResult> {
        let mut checks = Vec::new();

        checks.push(self.check_pipewire().await);
        checks.push(self.check_portal().await);
        checks.push(self.check_ffmpeg().await);
        checks.push(self.check_dbus().await);
        checks.push(self.check_notification().await);

        if let Some(backend_result) = self.check_backend().await {
            checks.push(backend_result);
        }

        checks.extend(self.check_output_directories().await);
        checks.extend(self.check_gstreamer_elements().await);
        checks.extend(self.check_pipewire_nodes().await);

        checks
    }

    pub async fn check_pipewire(&self) -> DoctorResult {
        let runtime_dir = env::var("XDG_RUNTIME_DIR").unwrap_or_default();
        let path = format!("{runtime_dir}/pipewire-0");
        let ok = Path::new(&path).exists();

        let details = if ok {
            None
        } else {
            Some(format!(
                "no socket in {} - is pipewire service running?",
                runtime_dir
            ))
        };

        DoctorResult {
            name: "PipeWire Socket".into(),
            ok,
            required: true,
            details,
        }
    }

    pub async fn check_dbus(&self) -> DoctorResult {
        let ok = std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok();
        let details = if ok {
            None
        } else {
            Some("DBUS_SESSION_BUS_ADDRESS env var missing".into())
        };

        DoctorResult {
            name: "D-Bus Session Bus".into(),
            ok,
            required: true,
            details,
        }
    }

    pub async fn check_portal(&self) -> DoctorResult {
        let name = "XDG Screencast Portal";
        match tokio::time::timeout(Duration::from_secs(XDG_PORTAL_TIMEOUT_S), Screencast::new())
            .await
        {
            // we got result
            Ok(result) => {
                if let Err(e) = result {
                    // error
                    DoctorResult {
                        name: name.into(),
                        ok: false,
                        required: true,
                        details: Some(e.to_string()),
                    }
                } else {
                    // success
                    DoctorResult {
                        name: name.into(),
                        ok: true,
                        required: true,
                        details: None,
                    }
                }
            }
            // timeout
            Err(_) => DoctorResult {
                name: name.into(),
                ok: false,
                required: true,
                details: Some("tokio time out - is xdg portal running?".to_string()),
            },
        }
    }

    pub async fn check_ffmpeg(&self) -> DoctorResult {
        let name = "FFmpeg";
        match Command::new("ffmpeg").arg("-version").output().await {
            Ok(result) => {
                if result.status.success() {
                    DoctorResult {
                        name: name.into(),
                        ok: true,
                        required: true,
                        details: None,
                    }
                } else {
                    DoctorResult {
                        name: name.into(),
                        ok: false,
                        required: true,
                        details: Some(result.status.to_string()),
                    }
                }
            }
            Err(_) => DoctorResult {
                name: name.into(),
                ok: false,
                required: true,
                details: Some("not found in $PATH".to_string()),
            },
        }
    }

    pub async fn check_backend(&self) -> Option<DoctorResult> {
        match self.video_codec.get_backend() {
            CodecType::VAAPI => {
                let ok = Path::new("/dev/dri/renderD128").exists();
                let details = if ok {
                    None
                } else {
                    Some("VAAPI backend '/dev/dri/renderD128' not found".to_string())
                };
                Some(DoctorResult {
                    name: "VAAPI Render Node".into(),
                    ok,
                    required: true,
                    details,
                })
            }
            CodecType::NVIDIA => {
                let ok = Path::new("/dev/nvidia0").exists();
                let details = if ok {
                    None
                } else {
                    Some("NVIDIA driver '/dev/nvidia0' not found".to_string())
                };
                Some(DoctorResult {
                    name: "NVIDIA Device ".into(),
                    ok,
                    required: true,
                    details,
                })
            }
            CodecType::Software => None,
        }
    }

    fn check_directory(&self, name: String, path: &Path) -> DoctorResult {
        if !path.exists() || !path.is_dir() {
            return DoctorResult {
                name,
                ok: false,
                required: true,
                details: Some(format!("{} missing or not a dir", path.display())),
            };
        }
        let probe_path = path.join(".test");
        match std::fs::write(&probe_path, b"") {
            Ok(_) => {
                std::fs::remove_file(probe_path).unwrap_or_default();
                DoctorResult {
                    name,
                    ok: true,
                    required: true,
                    details: None,
                }
            }
            Err(e) => DoctorResult {
                name,
                ok: false,
                required: true,
                details: Some(e.to_string()),
            },
        }
    }

    pub async fn check_output_directories(&self) -> Vec<DoctorResult> {
        let checks: Vec<(String, &Path)> = vec![
            ("Video Directory".into(), &self.clip_directory),
            ("Metadata Directory".into(), &self.metadata_directory),
            ("Preview Directory".into(), &self.preview_directory),
        ];

        checks
            .into_iter()
            .map(|c| self.check_directory(c.0, c.1))
            .collect()
    }

    fn check_element(&self, name: &str, required: bool) -> DoctorResult {
        let ok = gstreamer::ElementFactory::find(name).is_some();
        let details = if ok {
            None
        } else {
            Some("relevant gstreamer package missing".into())
        };

        DoctorResult {
            name: format!("GStreamer Element {}", name),
            ok,
            required,
            details,
        }
    }

    pub async fn check_gstreamer_elements(&self) -> Vec<DoctorResult> {
        gstreamer::init().unwrap_or_default();

        let element_checks: Vec<(&str, bool)> = vec![
            // default ones
            ("pipewiresrc", true),
            ("watchdog", true),
            ("queue", true),
            ("capsfilter", true),
            ("appsink", true),
            ("videoconvert", true),
            ("videoscale", true),
            ("videorate", true),
            // depends on config
            (self.video_codec.get_parser(), true),
            (self.video_codec.get_encoder(), true),
            (self.video_format.get_mux(), true),
            (self.audio_codec.get_parser(), true),
            (self.audio_codec.get_encoder(), true),
            // optional
            ("nvh264enc", false),
            ("vah264enc", false),
            ("x264enc", false),
        ];

        element_checks
            .into_iter()
            .map(|c| self.check_element(c.0, c.1))
            .collect()
    }

    pub async fn check_pipewire_nodes(&self) -> Vec<DoctorResult> {
        let pipewire_manager = match PipewireManager::new() {
            Err(e) => {
                return vec![DoctorResult {
                    name: "Pipewire Manager".into(),
                    ok: false,
                    required: true,
                    details: Some(e.to_string()),
                }];
            }
            Ok(m) => m,
        };

        let mut reciever = pipewire_manager.subscribe();

        let (microphone_node_name, background_node_name) =
            (self.microphone.0.clone(), self.background.0.clone());

        let state =
            match tokio::time::timeout(Duration::from_secs(DEFAULT_PIPEWIRE_TIMEOUT), async {
                loop {
                    let state = reciever.borrow().clone();

                    let mic_found = microphone_node_name.is_empty()
                        || state
                            .devices
                            .iter()
                            .any(|d| d.node_name == microphone_node_name);
                    let bg_found = background_node_name.is_empty()
                        || state
                            .devices
                            .iter()
                            .any(|d| d.node_name == background_node_name);

                    // Only return early if BOTH required nodes have been discovered
                    if mic_found && bg_found {
                        return state;
                    }

                    if reciever.changed().await.is_err() {
                        return state;
                    }
                }
            })
            .await
            {
                Ok(state) => state,
                Err(_) => pipewire_manager.current_state(),
            };

        let valid = |name: &str| state.devices.iter().any(|d| d.node_name == name);

        let mut results = Vec::new();

        if !microphone_node_name.is_empty() && valid(microphone_node_name.as_str()) {
            results.push(DoctorResult {
                name: format!("Microphone Node - {}", microphone_node_name),
                ok: true,
                required: self.microphone.1,
                details: None,
            });
        } else {
            results.push(DoctorResult {
                name: format!("Microphone Node - {}", microphone_node_name),
                ok: false,
                required: self.microphone.1,
                details: Some(format!("Invalid pipewire node: {}", microphone_node_name)),
            });
        }

        if !background_node_name.is_empty() && valid(background_node_name.as_str()) {
            results.push(DoctorResult {
                name: format!("Background Node - {}", background_node_name),
                ok: true,
                required: self.background.1,
                details: None,
            });
        } else {
            results.push(DoctorResult {
                name: format!("Background Node - {}", background_node_name),
                ok: false,
                required: self.background.1,
                details: Some(format!("Invalid pipewire node: {}", background_node_name)),
            });
        }

        results
    }

    pub async fn check_notification(&self) -> DoctorResult {
        match NotificationManager::test_notification(
            NotificationEvent::Test,
            "This is a test".into(),
        )
        .await
        {
            Err(e) => DoctorResult {
                name: "Notification Manager".into(),
                ok: false,
                required: false,
                details: Some(e.to_string()),
            },
            Ok(_) => DoctorResult {
                name: "Notification Manager".into(),
                ok: true,
                required: false,
                details: None,
            },
        }
    }
}
