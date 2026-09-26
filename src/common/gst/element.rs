use gstreamer::{
    Element, ElementFactory,
    glib::{self, value::ToValue},
};
use std::{borrow::Cow, path::PathBuf};
use wayclip_core::models::error::WayclipError;

use crate::DEFAULT_PREVIEW_BITRATE;

// --- Element Types ---

/// This enum will contain every standardised element identifier
#[derive(Clone, Debug)]
pub enum GStreamerElementType {
    X264Enc,
    H264Parse,
    DecodeBin,
    VideoConvert,
    VideoScale,
    MatroskaMux,
    CapsFilter { caps: gstreamer::Caps },
    SaveMux { mux: Cow<'static, str> },
    FileSink { location: PathBuf },
    SaveVideoParser { parser: Cow<'static, str> },
    SaveAudioParser { parser: Cow<'static, str> },
    FileSource { location: PathBuf },
}

impl GStreamerElementType {
    /// For every standardised element identifier, we assiggn properties such as factoryname, name &
    /// any additional properties
    pub fn get_element(&self) -> GStreamerElement {
        match self {
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
    name: Cow<'static, str>,
    // The value can either be strongly typed using glib::Value or just as string
    // e.g. tune="zerolatency"
    value: GStreamerElementPropertyValue,
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
