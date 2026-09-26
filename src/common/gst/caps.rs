use gstreamer::{
    Caps,
    glib::{self, value::ToSendValue},
};
use std::borrow::Cow;

// --- Caps ---

#[derive(Clone, Debug)]
pub enum GStreamerCapsType {
    VideoXRaw { width: Option<i32> },
}

impl GStreamerCapsType {
    pub fn get_caps(&self) -> GStreamerCaps {
        match self {
            Self::VideoXRaw { width } => {
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
                caps
            }
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
