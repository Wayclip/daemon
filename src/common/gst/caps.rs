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
        rate: Option<i32>,
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
                let mut fields = Vec::new();
                if let Some(w) = width {
                    fields.push(GStreamerCapsField::new("width", w.to_send_value()));
                }
                if let Some(h) = height {
                    fields.push(GStreamerCapsField::new("height", h.to_send_value()));
                }
                if let Some(f) = format {
                    fields.push(GStreamerCapsField::new(
                        "format",
                        f.to_string().to_send_value(),
                    ));
                }
                if let Some(f) = framerate {
                    fields.push(GStreamerCapsField::new("framerate", f.to_send_value()));
                }
                let features = memory.as_ref().map(|m| m.to_string().into());

                GStreamerCaps::new("video/x-raw", features, fields)
            }
            Self::AudioXRaw { rate, channels } => {
                let mut fields = vec![GStreamerCapsField::new(
                    "channels",
                    channels.to_send_value(),
                )];
                if let Some(r) = rate {
                    fields.push(GStreamerCapsField::new("rate", r.to_send_value()));
                }

                GStreamerCaps::new("audio/x-raw", None, fields)
            }
            Self::VideoXH264 => GStreamerCaps::new(
                "video/x-h264",
                None,
                vec![
                    GStreamerCapsField::new("stream-format", "byte-stream"),
                    GStreamerCapsField::new("alignment", "au"),
                ],
            ),
            Self::VideoXH265 => GStreamerCaps::new(
                "video/x-h265",
                None,
                vec![
                    GStreamerCapsField::new("stream-format", "byte-stream"),
                    GStreamerCapsField::new("alignment", "au"),
                ],
            ),
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
    pub fn new<N, V>(name: N, value: V) -> Self
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
