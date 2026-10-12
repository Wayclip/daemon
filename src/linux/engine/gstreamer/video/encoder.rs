use gstreamer::Element;
use wayclip_core::{
    models::error::WayclipError,
    settings::{
        UserSettings,
        recording::{CodecType, VideoCodec},
    },
};

use crate::common::gst::element::{
    GStreamerElement, GStreamerElementProperty, GStreamerElementPropertyValue,
};

pub struct VideoEncoderFactory;

impl VideoEncoderFactory {
    pub fn build_encoder(user_settings: &UserSettings) -> Result<Element, WayclipError> {
        let codec = &user_settings.recording.video.codec;
        let bitrate = user_settings.recording.video.bitrate_kbps.0;
        let gop = user_settings.recording.video.fps.0.max(1) as i32;

        match codec.get_backend() {
            CodecType::NVIDIA => Self::build_nvidia_pipeline(codec, bitrate, gop),
            CodecType::VAAPI => Self::build_vaapi_pipeline(codec, bitrate, gop),
            CodecType::Software => Self::build_software_pipeline(codec, bitrate, gop),
        }
    }

    fn build_nvidia_pipeline(
        codec: &VideoCodec,
        bitrate: u32,
        gop: i32,
    ) -> Result<Element, WayclipError> {
        GStreamerElement::new(
            codec.get_encoder(),
            vec![
                GStreamerElementProperty::new(
                    "bitrate",
                    GStreamerElementPropertyValue::typed(bitrate),
                ),
                GStreamerElementProperty::new(
                    "gop-size",
                    GStreamerElementPropertyValue::typed(gop),
                ),
                GStreamerElementProperty::new(
                    "rc-mode",
                    GStreamerElementPropertyValue::from_str("cbr"),
                ),
            ],
        )
        .build_element()
    }

    fn build_vaapi_pipeline(
        codec: &VideoCodec,
        bitrate: u32,
        gop: i32,
    ) -> Result<Element, WayclipError> {
        GStreamerElement::new(
            codec.get_encoder(),
            vec![
                GStreamerElementProperty::new(
                    "bitrate",
                    GStreamerElementPropertyValue::typed(bitrate),
                ),
                GStreamerElementProperty::new(
                    "key-int-max",
                    GStreamerElementPropertyValue::typed(gop),
                ),
            ],
        )
        .build_element()
    }

    fn build_software_pipeline(
        codec: &VideoCodec,
        bitrate: u32,
        gop: i32,
    ) -> Result<Element, WayclipError> {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(4);

        let properties = match codec {
            VideoCodec::H264(_) => vec![
                GStreamerElementProperty::new(
                    "bitrate",
                    GStreamerElementPropertyValue::typed(bitrate),
                ),
                GStreamerElementProperty::new(
                    "key-int-max",
                    GStreamerElementPropertyValue::typed(gop),
                ),
                GStreamerElementProperty::new(
                    "speed-preset",
                    GStreamerElementPropertyValue::from_str("ultrafast"),
                ),
                GStreamerElementProperty::new(
                    "tune",
                    GStreamerElementPropertyValue::from_str("zerolatency"),
                ),
                GStreamerElementProperty::new(
                    "threads",
                    GStreamerElementPropertyValue::typed(threads),
                ),
                GStreamerElementProperty::new(
                    "sliced-threads",
                    GStreamerElementPropertyValue::typed(true),
                ),
            ],
            VideoCodec::H265(_) => vec![
                GStreamerElementProperty::new(
                    "bitrate",
                    GStreamerElementPropertyValue::typed(bitrate),
                ),
                GStreamerElementProperty::new(
                    "key-int-max",
                    GStreamerElementPropertyValue::typed(gop),
                ),
                GStreamerElementProperty::new(
                    "speed-preset",
                    GStreamerElementPropertyValue::from_str("ultrafast"),
                ),
            ],
            // AV1 uses target-bitrate instead
            VideoCodec::AV1(_) => vec![
                GStreamerElementProperty::new(
                    "target-bitrate",
                    GStreamerElementPropertyValue::typed(bitrate),
                ),
                GStreamerElementProperty::new(
                    "keyframe-max-dist",
                    GStreamerElementPropertyValue::typed(gop),
                ),
            ],
        };

        GStreamerElement::new(codec.get_encoder(), properties).build_element()
    }
}
