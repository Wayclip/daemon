use crate::common::gst::{caps::GStreamerCapsType, element::GStreamerElementType};
use gstreamer::{
    Caps, Element, Pad,
    prelude::{ElementExt, ElementExtManual, GstObjectExt, PadExt},
};
use wayclip_core::models::error::WayclipError;

pub mod app;
pub mod caps;
pub mod element;
pub mod pipeline;

/// We define a custom wrapper to be able to construct and template gstreamer elements more easily
pub struct GStreamer;

impl GStreamer {
    // --- Elements ---
    pub fn build_element(element_type: GStreamerElementType) -> Result<Element, WayclipError> {
        let element = element_type.get_element();
        element.build_element()
    }

    // --- Caps ---

    pub fn build_caps(caps_type: GStreamerCapsType) -> Caps {
        let caps = caps_type.get_caps();
        caps.build_caps()
    }

    // --- Pads ---

    pub fn get_static_pad(element: &Element, pad_name: &str) -> Result<Pad, WayclipError> {
        element.static_pad(pad_name).ok_or_else(|| {
            WayclipError::Remux(
                format!(
                    "Pad '{}' not found on element '{}'",
                    pad_name,
                    element.name()
                )
                .into(),
            )
        })
    }

    pub fn request_pad(element: &Element, template: &str) -> Result<Pad, WayclipError> {
        element.request_pad_simple(template).ok_or_else(|| {
            WayclipError::Remux(
                format!(
                    "Could not request pad template '{}' on element '{}'",
                    template,
                    element.name()
                )
                .into(),
            )
        })
    }

    pub fn link_pads(src_pad: &Pad, sink_pad: &Pad) -> Result<(), WayclipError> {
        src_pad.link(sink_pad).map_err(|err| {
            let src_elem = src_pad
                .parent_element()
                .map(|e| e.name().to_string())
                .unwrap_or_else(|| "unknown".into());
            let sink_elem = sink_pad
                .parent_element()
                .map(|e| e.name().to_string())
                .unwrap_or_else(|| "unknown".into());

            WayclipError::Remux(
                format!(
                    "Failed to link pad '{}:{}' to '{}:{}': {:?}",
                    src_elem,
                    src_pad.name(),
                    sink_elem,
                    sink_pad.name(),
                    err
                )
                .into(),
            )
        })?;
        Ok(())
    }

    pub fn link_static_pads(
        src: &Element,
        src_pad_name: &str,
        sink: &Element,
        sink_pad_name: &str,
    ) -> Result<(), WayclipError> {
        let src_pad = Self::get_static_pad(src, src_pad_name)?;
        let sink_pad = Self::get_static_pad(sink, sink_pad_name)?;
        Self::link_pads(&src_pad, &sink_pad)
    }

    pub fn link_to_request_pad(
        src: &Element,
        src_pad_name: &str,
        dest: &Element,
        dest_pad_template: &str,
    ) -> Result<Pad, WayclipError> {
        let src_pad = Self::get_static_pad(src, src_pad_name)?;
        let sink_pad = Self::request_pad(dest, dest_pad_template)?;
        Self::link_pads(&src_pad, &sink_pad)?;
        Ok(sink_pad)
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
