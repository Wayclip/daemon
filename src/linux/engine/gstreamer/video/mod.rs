use gstreamer::Element;
use wayclip_core::{models::error::WayclipError, settings::UserSettings};

use crate::linux::engine::gstreamer::video::encoder::VideoEncoderFactory;
use crate::linux::engine::gstreamer::video::parser::VideoParserFactory;
use crate::linux::engine::gstreamer::video::preprocessor::VideoPreprocessFactory;
use crate::linux::engine::gstreamer::{
    DEFAULT_GST_LEAKY_DOWNSTREAM, DEFAULT_MAX_SIZE_BUFFER, DEFAULT_MAX_SIZE_BYTES,
    DEFAULT_MAX_SIZE_TIME_NS,
};
use crate::{
    common::gst::{GStreamer, app::DEFAULT_APPSRC_DO_TIMESTAMP, element::GStreamerElementType},
    linux::engine::pipewire::DaemonEngineConnectionData,
};

pub mod encoder;
pub mod parser;
pub mod preprocessor;

pub struct VideoBranchBuilder<'a> {
    user_settings: &'a UserSettings,
    connection_data: &'a DaemonEngineConnectionData,
}

impl<'a> VideoBranchBuilder<'a> {
    pub fn new(
        user_settings: &'a UserSettings,
        connection_data: &'a DaemonEngineConnectionData,
    ) -> Self {
        Self {
            user_settings,
            connection_data,
        }
    }

    pub fn build(self) -> Result<Vec<Element>, WayclipError> {
        let (file_descriptor, node_id) = self.connection_data.extract_data()?;

        let pipewire_src = GStreamer::build_element(GStreamerElementType::VideoPipewireSrc {
            do_timestamp: DEFAULT_APPSRC_DO_TIMESTAMP,
            fd: file_descriptor,
            path: node_id.into(),
            keepalive_ms: 1000 / self.user_settings.recording.video.fps.0 as i32,
        })?;

        let video_queue_1 = Self::build_queue()?;
        let video_queue_2 = Self::build_queue()?;
        // Although this is not really an audio queue, i just define audio queue as being queue with
        // no parameters :)
        // TODO: CHANGE
        let video_queue_3 = GStreamer::build_element(GStreamerElementType::AudioQueue)?;

        let (mut elements, pre_encode_caps) =
            VideoPreprocessFactory::build(pipewire_src, video_queue_1, self.user_settings)?;

        let encoder = VideoEncoderFactory::build_encoder(self.user_settings)?;
        let (parser, parser_caps) =
            VideoParserFactory::build(&self.user_settings.recording.video.codec)?;

        elements.push(pre_encode_caps);
        elements.push(video_queue_2);
        elements.push(encoder);
        elements.push(parser);

        if let Some(caps) = parser_caps {
            elements.push(caps);
        }

        elements.push(video_queue_3);
        Ok(elements)

        //self.pipeline.add_and_link(
        //    video_pipeline
        //        .iter()
        //        .collect::<Vec<&gstreamer::Element>>()
        //        .as_slice(),
        //)?;

        //let video_appsink = GStreamerApp::build_app_sink();
        //let video_appsink_ref = video_appsink.upcast_ref::<gstreamer::Element>();

        //let last = self
        //    .pipeline
        //    .last_element()
        //    .ok_or_else(|| WayclipError::Video("No last video element".into()))?;
        //self.pipeline.add(video_appsink_ref)?;

        //self.pipeline.link(&last, video_appsink_ref)?;

        //// TODO:
        ////self.set_appsink_callbacks(&video_appsink, ContentType::Video)?;

        //Ok(())
    }

    fn build_queue() -> Result<Element, WayclipError> {
        Ok(GStreamer::build_element(
            GStreamerElementType::VideoQueue {
                buffers: DEFAULT_MAX_SIZE_BUFFER,
                bytes: DEFAULT_MAX_SIZE_BYTES,
                time: DEFAULT_MAX_SIZE_TIME_NS,
                leaky: DEFAULT_GST_LEAKY_DOWNSTREAM.into(),
            },
        )?)
    }
}
