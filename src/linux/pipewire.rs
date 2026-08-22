use pipewire::context::ContextRc;
use pipewire::main_loop::MainLoopRc;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use wayclip_core::models::error::WayclipError;

pub const MAX_CONNECT_ATTEMPTS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipewireNodeType {
    Source,
    Sink,
    Unknown,
}

impl From<&str> for PipewireNodeType {
    fn from(value: &str) -> Self {
        match value {
            "Audio/Source" => Self::Source,
            "Audio/Sink" => Self::Sink,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PipewireDevice {
    pub node_type: PipewireNodeType,
    pub node_name: String,
    pub node_description: String,
    pub node_id: u32,
}

#[derive(Clone, Debug, Default)]
pub struct PipewireState {
    pub devices: Vec<PipewireDevice>,
    pub default_sink_node_name: Option<String>,
    pub default_source_node_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PipewireManager {
    state_rx: watch::Receiver<PipewireState>,
}

impl PipewireManager {
    pub fn new() -> Result<Self, WayclipError> {
        pipewire::init();

        let (state_tx, state_rx) = watch::channel(PipewireState::default());

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
                                "PipeWire connection failed on attempt {}: {}",
                                connection_attempts + 1,
                                error
                            );

                            connection_attempts += 1;
                            std::thread::sleep(std::time::Duration::from_millis(500));
                        }

                        Err(error) => {
                            return Err(WayclipError::Pipewire(
                                format!(
                                    "Failed to connect to PipeWire after {} attempts: {}",
                                    connection_attempts, error,
                                )
                                .into(),
                            ));
                        }
                    }
                };

                let registry = core
                    .get_registry_rc()
                    .map_err(|e| WayclipError::Pipewire(e.to_string().into()))?;

                let state = Arc::new(Mutex::new(PipewireState::default()));

                let listeners: Rc<RefCell<Vec<Box<dyn Any>>>> = Rc::new(RefCell::new(Vec::new()));

                let listeners_weak = Rc::downgrade(&listeners);

                let registry_for_metadata = registry.clone();
                let core_for_metadata = core.clone();

                let state_for_add = Arc::clone(&state);
                let state_tx_for_add = state_tx.clone();

                let state_for_remove = Arc::clone(&state);
                let state_tx_for_remove = state_tx.clone();

                let registry_listener = registry
                    .add_listener_local()
                    .global(move |global| {
                        log::trace!(
                            "PipeWire global: id={}, type={:?}, props={:?}",
                            global.id,
                            global.type_,
                            global.props
                        );

                        if global.type_ == pipewire::types::ObjectType::Node
                            && let Some(props) = global.props.as_ref()
                            && let Some(media_class) = props.get("media.class")
                            && let Some(node_name) = props.get("node.name")
                            && media_class.starts_with("Audio/")
                            && media_class != "Audio/Stream"
                        {
                            let node_description =
                                props.get("node.description").unwrap_or(node_name);

                            let mut state = match state_for_add.lock() {
                                Ok(guard) => guard,

                                Err(error) => {
                                    log::error!(
                                        "Could not acquire PipeWire state lock: {}",
                                        error
                                    );
                                    return;
                                }
                            };

                            if !state
                                .devices
                                .iter()
                                .any(|device| device.node_id == global.id)
                            {
                                state.devices.push(PipewireDevice {
                                    node_type: media_class.into(),
                                    node_name: node_name.to_owned(),
                                    node_description: node_description.to_owned(),
                                    node_id: global.id,
                                });

                                log::debug!(
                                    "Discovered audio node: id={}, class={}, name={}, description={}",
                                    global.id,
                                    media_class,
                                    node_name,
                                    node_description
                                );
                            }

                            let _ = state_tx_for_add.send(state.clone());
                        }

                        if global.type_ == pipewire::types::ObjectType::Metadata
                            && let Some(props) = global.props.as_ref()
                            && props.get("metadata.name") == Some("default")
                        {
                            let metadata = match registry_for_metadata
                                .bind::<pipewire::metadata::Metadata, _>(global)
                            {
                                Ok(metadata) => metadata,

                                Err(error) => {
                                    log::error!(
                                        "Failed to bind PipeWire default metadata: {}",
                                        error
                                    );
                                    return;
                                }
                            };

                            let state_for_metadata = Arc::clone(&state_for_add);
                            let state_tx_for_metadata = state_tx_for_add.clone();
                            let core_for_sync = core_for_metadata.clone();

                            let metadata_listener = metadata
                                .add_listener_local()
                                .property(move |_subject, key, _type, value| {
                                    let Some(key) = key else {
                                        return 0;
                                    };

                                    if key != "default.audio.sink"
                                        && key != "default.audio.source"
                                    {
                                        return 0;
                                    }

                                    let parsed_name = value.and_then(|value| {
                                        serde_json::from_str::<serde_json::Value>(value)
                                            .ok()
                                            .and_then(|json| {
                                                json.get("name")?
                                                    .as_str()
                                                    .map(ToOwned::to_owned)
                                            })
                                    });

                                    let Ok(mut state) = state_for_metadata.lock() else {
                                        log::error!(
                                            "Could not acquire PipeWire state lock while reading metadata"
                                        );
                                        return 0;
                                    };

                                    match key {
                                        "default.audio.sink" => {
                                            state.default_sink_node_name = parsed_name.clone();

                                            log::debug!(
                                                "PipeWire default sink changed: {:?}",
                                                parsed_name
                                            );
                                        }

                                        "default.audio.source" => {
                                            state.default_source_node_name = parsed_name.clone();

                                            log::debug!(
                                                "PipeWire default source changed: {:?}",
                                                parsed_name
                                            );
                                        }

                                        _ => {}
                                    }

                                    let _ = state_tx_for_metadata.send(state.clone());

                                    0
                                })
                                .register();

                            if let Some(listeners_rc) = listeners_weak.upgrade() {
                                let mut listeners = listeners_rc.borrow_mut();

                                listeners.push(Box::new(metadata_listener));
                                listeners.push(Box::new(metadata));
                            }

                            if let Err(error) = core_for_sync.sync(0) {
                                log::error!(
                                    "Failed to synchronize PipeWire metadata: {}",
                                    error
                                );
                            }
                        }
                    })
                    .global_remove(move |id| {
                        let Ok(mut state) = state_for_remove.lock() else {
                            log::error!(
                                "Could not acquire PipeWire state lock while removing node"
                            );
                            return;
                        };

                        let old_len = state.devices.len();

                        state.devices.retain(|device| device.node_id != id);

                        if state.devices.len() != old_len {
                            log::debug!("Removed PipeWire node with id={}", id);
                            let _ = state_tx_for_remove.send(state.clone());
                        }
                    })
                    .register();

                listeners.borrow_mut().push(Box::new(registry_listener));

                core.sync(0)
                    .map_err(|e| WayclipError::Pipewire(e.to_string().into()))?;

                mainloop.run();

                Ok(())
            };

            if let Err(error) = run_loop() {
                log::error!("PipeWire background thread stopped: {:?}", error);
            }
        });

        Ok(Self { state_rx })
    }

    pub fn get_node_id_from_node_name(&self, node_name: &str) -> Option<u32> {
        self.current_state()
            .devices
            .iter()
            .find(|device| device.node_name == node_name)
            .map(|device| device.node_id)
    }

    pub fn is_node_name_valid(&self, node_name: &str) -> bool {
        self.current_state()
            .devices
            .iter()
            .any(|device| device.node_name == node_name)
    }

    pub fn subscribe(&self) -> watch::Receiver<PipewireState> {
        self.state_rx.clone()
    }

    pub fn current_state(&self) -> PipewireState {
        self.state_rx.borrow().clone()
    }

    pub fn get_default_sink_name(&self) -> Option<String> {
        self.current_state().default_sink_node_name
    }

    pub fn get_default_source_name(&self) -> Option<String> {
        self.current_state().default_source_node_name
    }
}
