use pipewire::context::ContextRc;
use pipewire::main_loop::MainLoopRc;
use pipewire::spa::utils::result::AsyncSeq;
use serde::Deserialize;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;
use tokio::sync::watch;
use wayclip_core::models::error::WayclipError;

pub const MAX_CONNECT_ATTEMPTS: usize = 10;
pub const RETRY_INTERVAL: Duration = Duration::from_millis(500);
pub const INIT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipewireNodeType {
    Source,
    Sink,
    Unknown,
}

impl From<&str> for PipewireNodeType {
    fn from(value: &str) -> Self {
        if value.starts_with("Audio/Source") {
            Self::Source
        } else if value.starts_with("Audio/Sink") {
            Self::Sink
        } else {
            Self::Unknown
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipewireDevice {
    pub node_type: PipewireNodeType,
    pub node_name: String,
    pub node_description: String,
    pub node_id: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PipewireState {
    pub devices: Vec<PipewireDevice>,
    pub default_sink: Option<PipewireDevice>,
    pub default_source: Option<PipewireDevice>,
}

#[derive(Default)]
struct DefaultNames {
    sink: Option<String>,
    source: Option<String>,
}

#[derive(Deserialize)]
struct MetadataDeviceName {
    name: String,
}

#[derive(Clone, Debug)]
pub struct PipewireManager {
    state_receiver: watch::Receiver<PipewireState>,
}

impl PipewireManager {
    pub fn new() -> Result<Self, WayclipError> {
        pipewire::init();

        let (state_sender, state_receiver) = watch::channel(PipewireState::default());
        let (init_sender, init_receiver) = mpsc::sync_channel::<Result<(), WayclipError>>(1);

        std::thread::spawn(move || {
            let run_loop = || -> Result<(), WayclipError> {
                let mainloop = MainLoopRc::new(None)
                    .map_err(|e| WayclipError::Pipewire(e.to_string().into()))?;

                let context = ContextRc::new(&mainloop, None)
                    .map_err(|e| WayclipError::Pipewire(e.to_string().into()))?;

                let mut connection_attempts = 0;
                let core = loop {
                    match context.connect_rc(None) {
                        Ok(core) => break core,
                        Err(error) if connection_attempts < MAX_CONNECT_ATTEMPTS => {
                            log::warn!(
                                "PipeWire connection failed (attempt {}/{}): {}",
                                connection_attempts + 1,
                                MAX_CONNECT_ATTEMPTS,
                                error
                            );
                            connection_attempts += 1;
                            std::thread::sleep(RETRY_INTERVAL);
                        }
                        Err(error) => {
                            return Err(WayclipError::Pipewire(
                                format!(
                                    "Failed to connect to PipeWire after {} attempts: {}",
                                    connection_attempts, error
                                )
                                .into(),
                            ));
                        }
                    }
                };

                let registry = core
                    .get_registry_rc()
                    .map_err(|e| WayclipError::Pipewire(e.to_string().into()))?;

                let listeners: Rc<RefCell<Vec<Box<dyn Any>>>> = Rc::new(RefCell::new(Vec::new()));
                let listeners_weak = Rc::downgrade(&listeners);

                // Tracks target default node names from metadata
                let default_names = Rc::new(RefCell::new(DefaultNames::default()));

                let pending_seq: Rc<Cell<Option<AsyncSeq>>> = Rc::new(Cell::new(None));
                let init_signal = Rc::new(RefCell::new(Some(init_sender.clone())));

                let core_listener = core
                    .add_listener_local()
                    .done({
                        let pending_seq = Rc::clone(&pending_seq);
                        let init_signal = Rc::clone(&init_signal);
                        move |_id, seq| {
                            if pending_seq.get() != Some(seq) {
                                return;
                            }
                            pending_seq.set(None);

                            if let Some(sender) = init_signal.borrow_mut().take() {
                                let _ = sender.send(Ok(()));
                            }
                        }
                    })
                    .register();

                listeners.borrow_mut().push(Box::new(core_listener));

                let registry_for_metadata = registry.clone();
                let core_for_metadata = core.clone();
                let state_sender_global = state_sender.clone();
                let state_sender_remove = state_sender.clone();

                let registry_listener = registry
                    .add_listener_local()
                    .global({
                        let default_names = Rc::clone(&default_names);
                        let pending_seq = Rc::clone(&pending_seq);
                        move |global| {
                            if global.type_ == pipewire::types::ObjectType::Node
                                && let Some(props) = global.props.as_ref()
                                && let Some(media_class) = props.get("media.class")
                                && let Some(node_name) = props.get("node.name")
                                && media_class.starts_with("Audio/")
                                && !media_class.contains("Stream")
                            {
                                let node_description =
                                    props.get("node.description").unwrap_or(node_name);

                                let device = PipewireDevice {
                                    node_type: media_class.into(),
                                    node_name: node_name.to_owned(),
                                    node_description: node_description.to_owned(),
                                    node_id: global.id,
                                };

                                let names = default_names.borrow();
                                let is_default_sink = names.sink.as_deref() == Some(node_name);
                                let is_default_source = names.source.as_deref() == Some(node_name);
                                drop(names);

                                state_sender_global.send_if_modified(|state| {
                                    if state.devices.iter().any(|d| d.node_id == global.id) {
                                        return false;
                                    }

                                    if is_default_sink {
                                        state.default_sink = Some(device.clone());
                                    }
                                    if is_default_source {
                                        state.default_source = Some(device.clone());
                                    }

                                    state.devices.push(device);

                                    log::debug!(
                                        "Discovered audio node: id={}, class={}, name={}",
                                        global.id,
                                        media_class,
                                        node_name
                                    );
                                    true
                                });
                            }

                            if global.type_ == pipewire::types::ObjectType::Metadata
                                && let Some(props) = global.props.as_ref()
                                && props.get("metadata.name") == Some("default")
                            {
                                let metadata = match registry_for_metadata
                                    .bind::<pipewire::metadata::Metadata, _>(
                                    global,
                                ) {
                                    Ok(m) => m,
                                    Err(err) => {
                                        log::error!(
                                            "Failed to bind PipeWire default metadata: {err}"
                                        );
                                        return;
                                    }
                                };

                                let default_names_metadata = Rc::clone(&default_names);
                                let state_sender_metadata = state_sender_global.clone();

                                let metadata_listener = metadata
                                    .add_listener_local()
                                    .property(move |_subject, key, _type, value| {
                                        let Some(key) = key else { return 0 };
                                        if key != "default.audio.sink"
                                            && key != "default.audio.source"
                                        {
                                            return 0;
                                        }

                                        let parsed_name = value.and_then(|val| {
                                            serde_json::from_str::<MetadataDeviceName>(val)
                                                .ok()
                                                .map(|d| d.name)
                                        });

                                        match key {
                                            "default.audio.sink" => {
                                                default_names_metadata.borrow_mut().sink =
                                                    parsed_name.clone();
                                            }
                                            "default.audio.source" => {
                                                default_names_metadata.borrow_mut().source =
                                                    parsed_name.clone();
                                            }
                                            _ => return 0,
                                        }

                                        state_sender_metadata.send_if_modified(|state| {
                                            let target = match key {
                                                "default.audio.sink" => &mut state.default_sink,
                                                "default.audio.source" => &mut state.default_source,
                                                _ => return false,
                                            };

                                            let new_device =
                                                parsed_name.as_deref().and_then(|name| {
                                                    state
                                                        .devices
                                                        .iter()
                                                        .find(|d| d.node_name == name)
                                                        .cloned()
                                                });

                                            if *target != new_device {
                                                log::debug!(
                                                    "PipeWire {key} changed: {new_device:?}"
                                                );
                                                *target = new_device;
                                                true
                                            } else {
                                                false
                                            }
                                        });

                                        0
                                    })
                                    .register();

                                if let Some(listeners_rc) = listeners_weak.upgrade() {
                                    let mut l = listeners_rc.borrow_mut();
                                    l.push(Box::new(metadata_listener));
                                    l.push(Box::new(metadata));
                                }

                                match core_for_metadata.sync(0) {
                                    Ok(seq) => pending_seq.set(Some(seq)),
                                    Err(err) => {
                                        log::error!(
                                            "Failed to synchronize PipeWire metadata: {err}"
                                        )
                                    }
                                }
                            }
                        }
                    })
                    .global_remove(move |id| {
                        state_sender_remove.send_if_modified(|state| {
                            let mut modified = false;
                            let old_len = state.devices.len();
                            state.devices.retain(|d| d.node_id != id);
                            if state.devices.len() != old_len {
                                modified = true;
                                log::debug!("Removed PipeWire node id={id}");
                            }

                            if state.default_sink.as_ref().map(|d| d.node_id) == Some(id) {
                                state.default_sink = None;
                                modified = true;
                            }
                            if state.default_source.as_ref().map(|d| d.node_id) == Some(id) {
                                state.default_source = None;
                                modified = true;
                            }

                            modified
                        });
                    })
                    .register();

                listeners.borrow_mut().push(Box::new(registry_listener));

                let seq = core
                    .sync(0)
                    .map_err(|e| WayclipError::Pipewire(e.to_string().into()))?;
                pending_seq.set(Some(seq));

                mainloop.run();
                Ok(())
            };

            if let Err(error) = run_loop() {
                let _ = init_sender.send(Err(error));
            }
        });

        init_receiver.recv_timeout(INIT_TIMEOUT).map_err(|_| {
            WayclipError::Pipewire("PipeWire did not finish initial sync in time".into())
        })??;

        Ok(Self { state_receiver })
    }

    pub fn get_node_id_from_node_name(&self, node_name: &str) -> Option<u32> {
        self.state_receiver
            .borrow()
            .devices
            .iter()
            .find(|d| d.node_name == node_name)
            .map(|d| d.node_id)
    }

    pub fn is_node_name_valid(&self, node_name: &str, node_type: PipewireNodeType) -> bool {
        self.state_receiver
            .borrow()
            .devices
            .iter()
            .any(|d| d.node_name == node_name && d.node_type == node_type)
    }

    pub fn subscribe(&self) -> watch::Receiver<PipewireState> {
        self.state_receiver.clone()
    }

    pub fn current_state(&self) -> PipewireState {
        self.state_receiver.borrow().clone()
    }

    pub fn get_default_sink(&self) -> Option<PipewireDevice> {
        self.state_receiver.borrow().default_sink.clone()
    }

    pub fn get_default_source(&self) -> Option<PipewireDevice> {
        self.state_receiver.borrow().default_source.clone()
    }

    //pub fn get_default_sink_name(&self) -> Option<String> {
    //    self.state_receiver
    //        .borrow()
    //        .default_sink
    //        .as_ref()
    //        .map(|d| d.node_name.clone())
    //}

    //pub fn get_default_source_name(&self) -> Option<String> {
    //    self.state_receiver
    //        .borrow()
    //        .default_source
    //        .as_ref()
    //        .map(|d| d.node_name.clone())
    //}
}
