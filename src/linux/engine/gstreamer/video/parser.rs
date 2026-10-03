use gstreamer::Element;
use wayclip_core::{models::error::WayclipError, settings::recording::VideoCodec};

use crate::{
    common::gst::{
        GStreamer,
        caps::GStreamerCapsType,
        element::{
            GStreamerElement, GStreamerElementProperty, GStreamerElementPropertyValue,
            GStreamerElementType,
        },
    },
    linux::engine::gstreamer::DEFAULT_CONFIG_INTERVAL,
};

pub struct VideoParserFactory;

impl VideoParserFactory {
    pub fn build(codec: &VideoCodec) -> Result<(Element, Option<Element>), WayclipError> {
        match codec {
            VideoCodec::H264(_) => Self::build_annex_b_parser(codec, GStreamerCapsType::VideoXH264),
            VideoCodec::H265(_) => Self::build_annex_b_parser(codec, GStreamerCapsType::VideoXH265),
            VideoCodec::AV1(_) => {
                let parser = GStreamerElement::new(codec.get_parser(), vec![]).build_element()?;

                Ok((parser, None))
            }
        }
    }

    fn build_annex_b_parser(
        codec: &VideoCodec,
        caps_type: GStreamerCapsType,
    ) -> Result<(Element, Option<Element>), WayclipError> {
        let parser = GStreamerElement::new(
            codec.get_parser(),
            vec![GStreamerElementProperty {
                name: "config-interval".into(),
                value: GStreamerElementPropertyValue::Typed(DEFAULT_CONFIG_INTERVAL.into()),
            }],
        )
        .build_element()?;

        let caps = GStreamer::build_caps(caps_type);
        let caps_filter = GStreamer::build_element(GStreamerElementType::CapsFilter { caps })?;

        Ok((parser, Some(caps_filter)))
    }
}
