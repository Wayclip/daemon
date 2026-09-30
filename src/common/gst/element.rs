use gstreamer::{
    Element, ElementFactory, Structure,
    glib::{self, value::ToValue},
};
use std::{borrow::Cow, path::PathBuf};
use wayclip_core::models::error::WayclipError;

use crate::DEFAULT_PREVIEW_BITRATE;

// --- Element Types ---

/// This enum will contain every standardised element identifier
#[derive(Clone, Debug)]
pub enum GStreamerElementType {
    VAPostProc,
    GLUpload,
    GLColorScale,
    GLColorConvert,
    X264Enc,
    H264Parse,
    DecodeBin,
    VideoConvert,
    VideoScale,
    MatroskaMux,
    VideoRate,
    VideoQueue {
        buffers: u32,
        bytes: u32,
        time: u64,
        leaky: Cow<'static, str>,
    },
    AudioQueue,
    AudioConvert,
    AudioResample,
    CapsFilter {
        caps: gstreamer::Caps,
    },
    SaveMux {
        mux: Cow<'static, str>,
    },
    FileSink {
        location: PathBuf,
    },
    SaveVideoParser {
        parser: Cow<'static, str>,
    },
    SaveAudioParser {
        parser: Cow<'static, str>,
    },
    FileSource {
        location: PathBuf,
    },
    AudioPipewireSrc {
        do_timestamp: bool,
        target_object: Cow<'static, str>,
        sink: bool,
    },
    VideoPipewireSrc {
        do_timestamp: bool,
        fd: i32,
        path: Cow<'static, str>,
        keepalive_ms: i32,
    },
}

impl GStreamerElementType {
    /// For every standardised element identifier, we assiggn properties such as factoryname, name &
    /// any additional properties
    pub fn get_element(&self) -> GStreamerElement {
        match self {
            Self::VAPostProc => GStreamerElement {
                factoryname: "vapostproc".into(),
                ..Default::default()
            },
            Self::VideoRate => GStreamerElement {
                factoryname: "videorate".into(),
                ..Default::default()
            },
            Self::GLColorScale => GStreamerElement {
                factoryname: "glcolorscale".into(),
                ..Default::default()
            },
            Self::GLColorConvert => GStreamerElement {
                factoryname: "glcolorconvert".into(),
                ..Default::default()
            },
            Self::GLUpload => GStreamerElement {
                factoryname: "glupload".into(),
                ..Default::default()
            },
            Self::VideoQueue {
                buffers,
                bytes,
                time,
                leaky,
            } => GStreamerElement {
                factoryname: "queue".into(),
                properties: vec![
                    GStreamerElementProperty {
                        name: "max-size-buffers".into(),
                        value: GStreamerElementPropertyValue::Typed(buffers.into()),
                    },
                    GStreamerElementProperty {
                        name: "max-size-bytes".into(),
                        value: GStreamerElementPropertyValue::Typed(bytes.into()),
                    },
                    GStreamerElementProperty {
                        name: "max-size-time".into(),
                        value: GStreamerElementPropertyValue::Typed(time.into()),
                    },
                    GStreamerElementProperty {
                        name: "leaky".into(),
                        value: GStreamerElementPropertyValue::Serialized(leaky.to_owned()),
                    },
                ],
            },
            Self::AudioConvert => GStreamerElement {
                factoryname: "audioconvert".into(),
                ..Default::default()
            },
            Self::AudioResample => GStreamerElement {
                factoryname: "audioresample".into(),
                ..Default::default()
            },
            Self::VideoPipewireSrc {
                do_timestamp,
                fd,
                path,
                keepalive_ms,
            } => GStreamerElement {
                factoryname: "pipewiresrc".into(),
                properties: vec![
                    GStreamerElementProperty {
                        name: "do-timestamp".into(),
                        value: GStreamerElementPropertyValue::Typed(do_timestamp.into()),
                    },
                    GStreamerElementProperty {
                        name: "fd".into(),
                        value: GStreamerElementPropertyValue::Typed(fd.into()),
                    },
                    GStreamerElementProperty {
                        name: "path".into(),
                        value: GStreamerElementPropertyValue::Typed(path.to_value()),
                    },
                    GStreamerElementProperty {
                        name: "keepalive-time".into(),
                        value: GStreamerElementPropertyValue::Typed(keepalive_ms.into()),
                    },
                ],
            },
            Self::AudioPipewireSrc {
                do_timestamp,
                target_object,
                sink,
            } => {
                let structure = Structure::builder("properties")
                    .field("node.always-process", true)
                    .field("stream.capture.sink", sink);

                GStreamerElement {
                    factoryname: "pipewiresrc".into(),
                    properties: vec![
                        GStreamerElementProperty {
                            name: "do-timestamp".into(),
                            value: GStreamerElementPropertyValue::Typed(do_timestamp.into()),
                        },
                        GStreamerElementProperty {
                            name: "target-object".into(),
                            value: GStreamerElementPropertyValue::Typed(target_object.to_value()),
                        },
                        GStreamerElementProperty {
                            name: "stream-properties".into(),
                            value: GStreamerElementPropertyValue::Typed(structure.build().into()),
                        },
                    ],
                }
            }
            Self::MatroskaMux => GStreamerElement {
                factoryname: "matroskamux".into(),
                properties: vec![GStreamerElementProperty {
                    name: "streamable".into(),
                    value: GStreamerElementPropertyValue::Typed(true.into()),
                }],
            },
            Self::H264Parse => GStreamerElement {
                factoryname: "h264parse".into(),
                ..Default::default()
            },
            Self::X264Enc => GStreamerElement {
                factoryname: "x264enc".into(),
                properties: vec![
                    GStreamerElementProperty {
                        name: "tune".into(),
                        value: GStreamerElementPropertyValue::Serialized("zerolatency".into()),
                    },
                    GStreamerElementProperty {
                        name: "speed-preset".into(),
                        value: GStreamerElementPropertyValue::Serialized("veryfast".into()),
                    },
                    GStreamerElementProperty {
                        name: "bitrate".into(),
                        value: GStreamerElementPropertyValue::Typed(DEFAULT_PREVIEW_BITRATE.into()),
                    },
                ],
            },
            Self::DecodeBin => GStreamerElement {
                factoryname: "decodebin".into(),
                ..Default::default()
            },
            Self::VideoConvert => GStreamerElement {
                factoryname: "videoconvert".into(),
                ..Default::default()
            },
            Self::VideoScale => GStreamerElement {
                factoryname: "videoscale".into(),
                ..Default::default()
            },
            Self::SaveMux { mux } => GStreamerElement {
                factoryname: mux.clone(),
                ..Default::default()
            },
            Self::CapsFilter { caps } => GStreamerElement {
                factoryname: "capsfilter".into(),
                properties: vec![GStreamerElementProperty {
                    name: "caps".into(),
                    value: GStreamerElementPropertyValue::Typed(caps.into()),
                }],
            },
            Self::FileSink { location } => GStreamerElement {
                factoryname: "filesink".into(),
                properties: vec![GStreamerElementProperty {
                    name: "location".into(),
                    value: GStreamerElementPropertyValue::Typed(location.to_value()),
                }],
            },
            Self::SaveVideoParser { parser } => GStreamerElement {
                factoryname: parser.clone(),
                ..Default::default()
            },
            Self::SaveAudioParser { parser } => GStreamerElement {
                factoryname: parser.clone(),
                ..Default::default()
            },
            Self::AudioQueue => GStreamerElement {
                factoryname: "queue".into(),
                ..Default::default()
            },
            Self::FileSource { location } => GStreamerElement {
                factoryname: "filesrc".into(),
                properties: vec![GStreamerElementProperty {
                    name: "location".into(),
                    value: GStreamerElementPropertyValue::Typed(location.to_value()),
                }],
            },
        }
    }
}

// --- Elements ---

/// A custom wrapper around a gstreamer element
#[derive(Clone, Debug, Default)]
pub struct GStreamerElement {
    pub factoryname: Cow<'static, str>,
    pub properties: Vec<GStreamerElementProperty>,
}

impl GStreamerElement {
    /// A standardised method so we can keep our fields private & also convert to Cow<> (Copy on
    /// borrow)
    pub fn new<F>(factoryname: F, properties: Vec<GStreamerElementProperty>) -> Self
    where
        F: Into<Cow<'static, str>>,
    {
        Self {
            factoryname: factoryname.into(),
            properties,
        }
    }

    pub fn build_element(&self) -> Result<Element, WayclipError> {
        let mut make = ElementFactory::make(self.factoryname.as_ref());
        for prop in &self.properties {
            match &prop.value {
                GStreamerElementPropertyValue::Typed(value) => {
                    make = make.property(&prop.name, value)
                }
                GStreamerElementPropertyValue::Serialized(value) => {
                    make = make.property_from_str(&prop.name, value)
                }
            }
        }
        make.build().map_err(WayclipError::from)
    }
}

#[derive(Clone, Debug)]
pub struct GStreamerElementProperty {
    pub name: Cow<'static, str>,
    // The value can either be strongly typed using glib::Value or just as string
    // e.g. tune="zerolatency"
    pub value: GStreamerElementPropertyValue,
}

#[derive(Clone, Debug)]
pub enum GStreamerElementPropertyValue {
    Typed(glib::Value),
    Serialized(Cow<'static, str>),
}

impl GStreamerElementPropertyValue {
    pub fn typed<V: Into<glib::Value>>(value: V) -> Self {
        Self::Typed(value.into())
    }

    pub fn from_str<V: Into<Cow<'static, str>>>(value: V) -> Self {
        Self::Serialized(value.into())
    }
}

impl GStreamerElementProperty {
    fn new<N>(name: N, value: GStreamerElementPropertyValue) -> Self
    where
        N: Into<Cow<'static, str>>,
    {
        Self {
            name: name.into(),
            value,
        }
    }
}
