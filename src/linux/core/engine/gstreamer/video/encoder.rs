use gstreamer::Element;
use wayclip_core::{
    models::error::WayclipError,
    settings::{
        UserSettings,
        recording::{CodecType, VideoCodec},
    },
};

use crate::{
    common::gst::element::{
        GStreamerElement, GStreamerElementProperty, GStreamerElementPropertyValue,
    },
    linux::core::engine::gstreamer::{DEFAULT_GOP_SIZE, DEFAULT_KEYFRAME_PERIOD},
};

pub struct VideoEncoderFactory;

impl VideoEncoderFactory {
    pub fn build_encoder(user_settings: &UserSettings) -> Result<Element, WayclipError> {
        let codec = &user_settings.recording.video.codec;
        let bitrate = user_settings.recording.video.bitrate_kbps.0;

        match codec.get_backend() {
            CodecType::NVIDIA => Self::build_nvidia_pipeline(codec, bitrate),
            CodecType::VAAPI => Self::build_vaapi_pipeline(codec, bitrate),
            CodecType::Software => Self::build_software_pipeline(codec, bitrate),
        }
    }

    fn build_nvidia_pipeline(codec: &VideoCodec, bitrate: u32) -> Result<Element, WayclipError> {
        GStreamerElement::new(
            codec.get_encoder(),
            vec![
                GStreamerElementProperty {
                    name: "bitrate".into(),
                    value: GStreamerElementPropertyValue::Typed(bitrate.into()),
                },
                GStreamerElementProperty {
                    name: "gop-size".into(),
                    value: GStreamerElementPropertyValue::Typed(DEFAULT_GOP_SIZE.into()),
                },
                GStreamerElementProperty {
                    name: "rc-mode".into(),
                    value: GStreamerElementPropertyValue::Serialized("cbr".into()),
                },
            ],
        )
        .build_element()
    }

    fn build_vaapi_pipeline(codec: &VideoCodec, bitrate: u32) -> Result<Element, WayclipError> {
        GStreamerElement::new(
            codec.get_encoder(),
            vec![
                GStreamerElementProperty {
                    name: "bitrate".into(),
                    value: GStreamerElementPropertyValue::Typed(bitrate.into()),
                },
                GStreamerElementProperty {
                    name: "key-int-max".into(),
                    value: GStreamerElementPropertyValue::Typed(DEFAULT_KEYFRAME_PERIOD.into()),
                },
            ],
        )
        .build_element()
    }

    fn build_software_pipeline(codec: &VideoCodec, bitrate: u32) -> Result<Element, WayclipError> {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(4);

        let properties = match codec {
            VideoCodec::H264(_) => vec![
                GStreamerElementProperty {
                    name: "bitrate".into(),
                    value: GStreamerElementPropertyValue::Typed(bitrate.into()),
                },
                GStreamerElementProperty {
                    name: "key-int-max".into(),
                    value: GStreamerElementPropertyValue::Typed(DEFAULT_KEYFRAME_PERIOD.into()),
                },
                GStreamerElementProperty {
                    name: "speed-preset".into(),
                    value: GStreamerElementPropertyValue::Serialized("ultrafast".into()),
                },
                GStreamerElementProperty {
                    name: "tune".into(),
                    value: GStreamerElementPropertyValue::Serialized("zerolatency".into()),
                },
                GStreamerElementProperty {
                    name: "threads".into(),
                    value: GStreamerElementPropertyValue::Typed(threads.into()),
                },
                GStreamerElementProperty {
                    name: "sliced-threads".into(),
                    value: GStreamerElementPropertyValue::Typed(true.into()),
                },
            ],
            VideoCodec::H265(_) => vec![
                GStreamerElementProperty {
                    name: "bitrate".into(),
                    value: GStreamerElementPropertyValue::Typed(bitrate.into()),
                },
                GStreamerElementProperty {
                    name: "key-int-max".into(),
                    value: GStreamerElementPropertyValue::Typed(DEFAULT_KEYFRAME_PERIOD.into()),
                },
                GStreamerElementProperty {
                    name: "speed-preset".into(),
                    value: GStreamerElementPropertyValue::Serialized("ultrafast".into()),
                },
            ],
            // AV1 uses target-bitrate instead
            VideoCodec::AV1(_) => vec![GStreamerElementProperty {
                name: "target-bitrate".into(),
                value: GStreamerElementPropertyValue::Typed(bitrate.into()),
            }],
        };

        GStreamerElement::new(codec.get_encoder(), properties).build_element()
    }
}
