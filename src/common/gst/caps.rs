use gstreamer::{
    Caps, Fraction,
    glib::{self, value::ToSendValue},
};
use std::borrow::Cow;
use strum_macros::Display;

// --- Caps ---

#[derive(Clone, Debug, Display)]
pub enum VideoXRawMemory {
    #[strum(serialize = "memory:DMABuf")]
    DMABuf,
    #[strum(serialize = "memory:GLMemory")]
    GLMemory,
    #[strum(serialize = "memory:VAMemory")]
    VAMemory,
}

#[derive(Clone, Debug, Display)]
pub enum VideoXRawFormat {
    RGBA,
    NV12,
    I420,
}

#[derive(Clone, Debug)]
pub enum GStreamerCapsType {
    VideoXRaw {
        width: Option<i32>,
        height: Option<i32>,
        framerate: Option<Fraction>,
        format: Option<VideoXRawFormat>,
        memory: Option<VideoXRawMemory>,
    },
    AudioXRaw {
        rate: i32,
        channels: i32,
    },
    VideoXH264,
    VideoXH265,
}

impl GStreamerCapsType {
    pub fn get_caps(&self) -> GStreamerCaps {
        match self {
            Self::VideoXRaw {
                width,
                height,
                framerate,
                memory,
                format,
            } => {
                let mut caps = GStreamerCaps {
                    name: "video/x-raw".into(),
                    features: None,
                    ..Default::default()
                };
                if let Some(w) = width {
                    caps.fields.push(GStreamerCapsField {
                        name: "width".into(),
                        value: w.to_send_value(),
                    });
                }
                if let Some(h) = height {
                    caps.fields.push(GStreamerCapsField {
                        name: "height".into(),
                        value: h.to_send_value(),
                    });
                }
                if let Some(f) = format {
                    caps.fields.push(GStreamerCapsField {
                        name: "format".into(),
                        value: f.to_string().to_send_value(),
                    });
                }
                if let Some(f) = framerate {
                    caps.fields.push(GStreamerCapsField {
                        name: "framerate".into(),
                        value: f.to_send_value(),
                    });
                }
                if let Some(m) = memory {
                    caps.features = Some(m.to_string().into())
                }
                caps
            }
            Self::AudioXRaw { rate, channels } => GStreamerCaps {
                name: "audio/x-raw".into(),
                features: None,
                fields: vec![
                    GStreamerCapsField {
                        name: "rate".into(),
                        value: rate.into(),
                    },
                    GStreamerCapsField {
                        name: "channels".into(),
                        value: channels.into(),
                    },
                ],
            },
            Self::VideoXH264 => GStreamerCaps {
                name: "video/x-h264".into(),
                features: None,
                fields: vec![
                    GStreamerCapsField {
                        name: "stream-format".into(),
                        value: "byte-stream".into(),
                    },
                    GStreamerCapsField {
                        name: "alignment".into(),
                        value: "au".into(),
                    },
                ],
            },
            Self::VideoXH265 => GStreamerCaps {
                name: "video/x-h265".into(),
                features: None,
                fields: vec![
                    GStreamerCapsField {
                        name: "stream-format".into(),
                        value: "byte-stream".into(),
                    },
                    GStreamerCapsField {
                        name: "alignment".into(),
                        value: "au".into(),
                    },
                ],
            },
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GStreamerCaps {
    name: Cow<'static, str>,
    features: Option<Cow<'static, str>>,
    fields: Vec<GStreamerCapsField>,
}

impl GStreamerCaps {
    pub fn new<N>(
        name: N,
        features: Option<Cow<'static, str>>,
        fields: Vec<GStreamerCapsField>,
    ) -> Self
    where
        N: Into<Cow<'static, str>>,
    {
        Self {
            name: name.into(),
            features,
            fields,
        }
    }

    pub fn build_caps(&self) -> Caps {
        match self.features {
            Some(ref features) => {
                let mut builder =
                    gstreamer::Caps::builder(self.name.as_ref()).features([features.as_ref()]);

                for field in &self.fields {
                    builder = builder.field(field.name.as_ref(), field.value.clone());
                }
                builder.build()
            }
            None => {
                let mut builder = gstreamer::Caps::builder(self.name.as_ref());

                for field in &self.fields {
                    builder = builder.field(field.name.as_ref(), field.value.clone());
                }
                builder.build()
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct GStreamerCapsField {
    name: Cow<'static, str>,
    value: glib::SendValue,
}

impl GStreamerCapsField {
    fn new<N, V>(name: N, value: V) -> Self
    where
        N: Into<Cow<'static, str>>,
        V: Into<glib::SendValue>,
    {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}
