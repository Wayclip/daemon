use crate::common::gst::{caps::GStreamerCapsType, element::GStreamerElementType};
use gstreamer::{
    Caps, Element,
    prelude::{ElementExt, PadExt},
};
use wayclip_core::models::error::WayclipError;

pub mod app;
pub mod caps;
pub mod element;
pub mod pipeline;

/// We define a custom wrapper to be able to construct and template gstreamer elements more easily
pub struct GStreamer;

impl GStreamer {
    pub fn build_element(element_type: GStreamerElementType) -> Result<Element, WayclipError> {
        let element = element_type.get_element();
        element.build_element()
    }

    pub fn build_caps(caps_type: GStreamerCapsType) -> Caps {
        let caps = caps_type.get_caps();
        caps.build_caps()
    }

    pub fn link_dynamic_pad(src: &Element, sink: Element, prefix: &'static str) {
        src.connect_pad_added(move |_, src_pad| {
            let Some(caps) = src_pad.current_caps() else {
                return;
            };
            let Some(structure) = caps.structure(0) else {
                return;
            };

            if structure.name().starts_with(prefix)
                && let Some(sink_pad) = sink.static_pad("sink")
                && !sink_pad.is_linked()
            {
                src_pad.link(&sink_pad).ok();
            }
        });
    }
}
