use gstreamer::{
    Element, ElementFactory, Structure,
    glib::{self, value::ToValue},
};
use std::{borrow::Cow, path::PathBuf};
use wayclip_core::models::error::WayclipError;

use crate::common::video::preview::DEFAULT_PREVIEW_BITRATE;

// --- Element Types ---

/// This enum will contain every standardised element identifier
#[derive(Clone, Debug)]
pub enum GStreamerElementType {
    AudioMixer,
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
            Self::AudioMixer => GStreamerElement::new(
                "audiomixer",
                vec![
                    GStreamerElementProperty::new(
                        "start-time-selection",
                        GStreamerElementPropertyValue::from_str("zero"),
                    ),
                    GStreamerElementProperty::new(
                        "ignore-inactive-pads",
                        GStreamerElementPropertyValue::typed(true),
                    ),
                ],
            ),
            Self::VAPostProc => GStreamerElement::new("vapostproc", vec![]),
            Self::VideoRate => GStreamerElement::new("videorate", vec![]),
            Self::GLColorScale => GStreamerElement::new("glcolorscale", vec![]),
            Self::GLColorConvert => GStreamerElement::new("glcolorconvert", vec![]),
            Self::GLUpload => GStreamerElement::new("glupload", vec![]),
            Self::VideoQueue {
                buffers,
                bytes,
                time,
                leaky,
            } => GStreamerElement::new(
                "queue",
                vec![
                    GStreamerElementProperty::new(
                        "max-size-buffers",
                        GStreamerElementPropertyValue::typed(*buffers),
                    ),
                    GStreamerElementProperty::new(
                        "max-size-bytes",
                        GStreamerElementPropertyValue::typed(*bytes),
                    ),
                    GStreamerElementProperty::new(
                        "max-size-time",
                        GStreamerElementPropertyValue::typed(*time),
                    ),
                    GStreamerElementProperty::new(
                        "leaky",
                        GStreamerElementPropertyValue::from_str(leaky.clone()),
                    ),
                ],
            ),
            Self::AudioConvert => GStreamerElement::new("audioconvert", vec![]),
            Self::AudioResample => GStreamerElement::new("audioresample", vec![]),
            Self::VideoPipewireSrc {
                do_timestamp,
                fd,
                path,
                keepalive_ms,
            } => GStreamerElement::new(
                "pipewiresrc",
                vec![
                    GStreamerElementProperty::new(
                        "do-timestamp",
                        GStreamerElementPropertyValue::typed(*do_timestamp),
                    ),
                    GStreamerElementProperty::new("fd", GStreamerElementPropertyValue::typed(*fd)),
                    GStreamerElementProperty::new(
                        "path",
                        GStreamerElementPropertyValue::typed(path.to_value()),
                    ),
                    GStreamerElementProperty::new(
                        "keepalive-time",
                        GStreamerElementPropertyValue::typed(*keepalive_ms),
                    ),
                ],
            ),
            Self::AudioPipewireSrc {
                do_timestamp,
                target_object,
                sink,
            } => {
                let structure = Structure::builder("properties")
                    .field("node.always-process", true)
                    .field("stream.capture.sink", sink);

                GStreamerElement::new(
                    "pipewiresrc",
                    vec![
                        GStreamerElementProperty::new(
                            "do-timestamp",
                            GStreamerElementPropertyValue::typed(*do_timestamp),
                        ),
                        GStreamerElementProperty::new(
                            "target-object",
                            GStreamerElementPropertyValue::typed(target_object.to_value()),
                        ),
                        GStreamerElementProperty::new(
                            "stream-properties",
                            GStreamerElementPropertyValue::typed(structure.build()),
                        ),
                    ],
                )
            }
            Self::MatroskaMux => GStreamerElement::new(
                "matroskamux",
                vec![GStreamerElementProperty::new(
                    "streamable",
                    GStreamerElementPropertyValue::typed(true),
                )],
            ),
            Self::H264Parse => GStreamerElement::new("h264parse", vec![]),
            Self::X264Enc => GStreamerElement::new(
                "x264enc",
                vec![
                    GStreamerElementProperty::new(
                        "tune",
                        GStreamerElementPropertyValue::from_str("zerolatency"),
                    ),
                    GStreamerElementProperty::new(
                        "speed-preset",
                        GStreamerElementPropertyValue::from_str("veryfast"),
                    ),
                    GStreamerElementProperty::new(
                        "bitrate",
                        GStreamerElementPropertyValue::typed(DEFAULT_PREVIEW_BITRATE),
                    ),
                ],
            ),
            Self::DecodeBin => GStreamerElement::new("decodebin", vec![]),
            Self::VideoConvert => GStreamerElement::new("videoconvert", vec![]),
            Self::VideoScale => GStreamerElement::new("videoscale", vec![]),
            Self::SaveMux { mux } => GStreamerElement::new(mux.clone(), vec![]),
            Self::CapsFilter { caps } => GStreamerElement::new(
                "capsfilter",
                vec![GStreamerElementProperty::new(
                    "caps",
                    GStreamerElementPropertyValue::typed(caps.clone()),
                )],
            ),
            Self::FileSink { location } => GStreamerElement::new(
                "filesink",
                vec![GStreamerElementProperty::new(
                    "location",
                    GStreamerElementPropertyValue::typed(location.to_value()),
                )],
            ),
            Self::SaveVideoParser { parser } => GStreamerElement::new(parser.clone(), vec![]),
            Self::SaveAudioParser { parser } => GStreamerElement::new(parser.clone(), vec![]),
            Self::AudioQueue => GStreamerElement::new("queue", vec![]),
            Self::FileSource { location } => GStreamerElement::new(
                "filesrc",
                vec![GStreamerElementProperty::new(
                    "location",
                    GStreamerElementPropertyValue::typed(location.to_value()),
                )],
            ),
        }
    }
}

// --- Elements ---

/// A custom wrapper around a gstreamer element
#[derive(Clone, Debug, Default)]
pub struct GStreamerElement {
    factoryname: Cow<'static, str>,
    properties: Vec<GStreamerElementProperty>,
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

impl GStreamerElementProperty {
    pub fn new<N>(name: N, value: GStreamerElementPropertyValue) -> Self
    where
        N: Into<Cow<'static, str>>,
    {
        Self {
            name: name.into(),
            value,
        }
    }
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
