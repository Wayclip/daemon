use crate::common::ring::ContentType;
use crate::linux::core::{
    DEFAULT_APPSINK_DROP, DEFAULT_APPSINK_MAX_BUFFERS, DEFAULT_APPSINK_SYNC,
    DEFAULT_PIPEWIRE_DO_TIMESTAMP, DaemonCore,
};
use gstreamer::glib::object::Cast;
use gstreamer::prelude::{ElementExt, ElementExtManual, GstBinExt, ObjectExt, PadExtManual};
use std::os::fd::AsRawFd;
use wayclip_core::models::error::WayclipError;
use wayclip_core::settings::recording::{CodecType, VideoCodec};

const DEFAULT_MAX_SIZE_BUFFER: u32 = 0;
const DEFAULT_CONFIG_INTERVAL: i32 = 1;
const DEFAULT_GST_LEAKY_DOWNSTREAM: &str = "2";
const DEFAULT_MAX_SIZE_BYTES: u32 = 0;
const DEFAULT_MAX_SIZE_TIME_NS: u64 = 500000000;
const DEFAULT_GOP_SIZE: i32 = 30;
const DEFAULT_KEYFRAME_PERIOD: u32 = 30;
const DEFAULT_WATCHDOG_TIMEOUT_MS: i32 = 5000;

pub type ParserFilter = (gstreamer::Element, Option<gstreamer::Element>);

impl DaemonCore {
    fn get_parser_and_filter(&self, codec: &VideoCodec) -> Result<ParserFilter, WayclipError> {
        match codec {
            VideoCodec::H264(_) => {
                let parser = gstreamer::ElementFactory::make(codec.get_parser())
                    .property("config-interval", DEFAULT_CONFIG_INTERVAL)
                    .name("video_h264_parser")
                    .build()?;
                let caps_filter = gstreamer::ElementFactory::make("capsfilter")
                    .name("video_h264_annexb_caps")
                    .build()?;
                caps_filter.set_property(
                    "caps",
                    gstreamer::Caps::builder("video/x-h264")
                        .field("stream-format", "byte-stream")
                        .field("alignment", "au")
                        .build(),
                );
                Ok((parser, Some(caps_filter)))
            }
            VideoCodec::H265(_) => {
                let parser = gstreamer::ElementFactory::make(codec.get_parser())
                    .property("config-interval", DEFAULT_CONFIG_INTERVAL)
                    .name("video_h265_parser")
                    .build()?;
                let caps_filter = gstreamer::ElementFactory::make("capsfilter")
                    .name("video_h265_annexb_caps")
                    .build()?;
                caps_filter.set_property(
                    "caps",
                    gstreamer::Caps::builder("video/x-h265")
                        .field("stream-format", "byte-stream")
                        .field("alignment", "au")
                        .build(),
                );
                Ok((parser, Some(caps_filter)))
            }
            VideoCodec::AV1(_) => {
                let parser = gstreamer::ElementFactory::make(codec.get_parser())
                    .name("video_av1_parser")
                    .build()?;
                Ok((parser, None))
            }
        }
    }

    pub fn build_video_pipeline(
        &self,
        pipeline: &gstreamer::Pipeline,
        codec: VideoCodec,
        resolution: (u64, u64),
        fps: u64,
        video_bitrate_kbps: u64,
    ) -> Result<(), WayclipError> {
        if self.pipewire_node_id.is_none() || self.pipewire_file_descriptor.is_none() {
            return Err(WayclipError::Video(
                "No node ID or FD was aquired by the daemon".into(),
            ));
        }

        let fd = self
            .pipewire_file_descriptor
            .as_ref()
            .ok_or_else(|| WayclipError::Video("Could not get fd_ref".into()))?
            .as_raw_fd();
        let id = self
            .pipewire_node_id
            .clone()
            .ok_or_else(|| WayclipError::Video("No node id present".into()))?;

        log::debug!("Using fd: {} and id: {}", fd, id);

        let video_pipewiresrc = gstreamer::ElementFactory::make("pipewiresrc")
            .property("do-timestamp", DEFAULT_PIPEWIRE_DO_TIMESTAMP)
            .property("fd", fd)
            .property("path", id)
            .name("video_pipewiresrc")
            .build()?;

        // this is just a static pad so we can verify once what the upstream caps is
        if let Some(src_pad) = video_pipewiresrc.static_pad("src") {
            src_pad.add_probe(gstreamer::PadProbeType::EVENT_DOWNSTREAM, |_, info| {
                if let Some(gstreamer::PadProbeData::Event(ref event)) = info.data
                    && let gstreamer::EventView::Caps(caps_event) = event.view()
                {
                    let caps = caps_event.caps();

                    log::info!("Negotiated initial caps: {:?}", caps);

                    return gstreamer::PadProbeReturn::Remove;
                }
                gstreamer::PadProbeReturn::Ok
            });
        }

        // decided to move all queus and watchdogs to top
        let video_watchdog = gstreamer::ElementFactory::make("watchdog")
            .property("timeout", DEFAULT_WATCHDOG_TIMEOUT_MS)
            .name("video_watchdog")
            .build()?;

        let video_queue_1 = gstreamer::ElementFactory::make("queue")
            .property("max-size-buffers", DEFAULT_MAX_SIZE_BUFFER)
            .property("max-size-bytes", DEFAULT_MAX_SIZE_BYTES)
            .property("max-size-time", DEFAULT_MAX_SIZE_TIME_NS)
            .property_from_str("leaky", DEFAULT_GST_LEAKY_DOWNSTREAM)
            .name("video_queue_1")
            .build()?;

        let video_queue_2 = gstreamer::ElementFactory::make("queue")
            .property("max-size-buffers", DEFAULT_MAX_SIZE_BUFFER)
            .property("max-size-bytes", DEFAULT_MAX_SIZE_BYTES)
            .property("max-size-time", DEFAULT_MAX_SIZE_TIME_NS)
            .property_from_str("leaky", DEFAULT_GST_LEAKY_DOWNSTREAM)
            .name("video_queue_2")
            .build()?;

        // couldve used codec.get_parser(), but AV1 dont support config-interval..
        // + also now we have a caps filter

        let (parser, parser_annexb_caps_filter) = self.get_parser_and_filter(&codec)?;

        let (mut video_pipeline, pre_encode_caps_filter): (
            Vec<gstreamer::Element>,
            gstreamer::Element,
        ) = match codec.get_backend() {
            CodecType::NVIDIA => {
                // for nvidia we have DMABuf -> glupload -> GLMemory -> format + colorconvert ->
                // GLMemory NV12
                let caps_filter_pipewiresrc = gstreamer::ElementFactory::make("capsfilter")
                    .name("video_pipewiresrc_caps")
                    .build()?;

                let dma_caps = gstreamer::Caps::builder("video/x-raw")
                    .features(["memory:DMABuf"])
                    // removed since crashes (we handle res downstream)
                    //.field("width", resolution.0 as i32)
                    //.field("height", resolution.1 as i32)
                    .build();

                caps_filter_pipewiresrc.set_property("caps", dma_caps);

                let glupload = gstreamer::ElementFactory::make("glupload")
                    .name("video_glupload")
                    .build()?;

                let caps_filter_glupload = gstreamer::ElementFactory::make("capsfilter")
                    .name("video_glupload_caps")
                    .build()?;
                caps_filter_glupload.set_property(
                    "caps",
                    gstreamer::Caps::builder("video/x-raw")
                        .features(["memory:GLMemory"])
                        .build(),
                );

                let glcolorscale = gstreamer::ElementFactory::make("glcolorscale")
                    .name("video_glcolorscale")
                    .build()?;

                let caps_filter_glscale = gstreamer::ElementFactory::make("capsfilter")
                    .name("video_glscale_caps")
                    .build()?;
                caps_filter_glscale.set_property(
                    "caps",
                    gstreamer::Caps::builder("video/x-raw")
                        .features(["memory:GLMemory"])
                        .field("format", "RGBA")
                        .field("width", resolution.0 as i32)
                        .field("height", resolution.1 as i32)
                        .build(),
                );

                let glcolorconvert = gstreamer::ElementFactory::make("glcolorconvert")
                    .name("video_glcolorconvert")
                    .build()?;

                let videorate = gstreamer::ElementFactory::make("videorate")
                    .name("video_rate")
                    .build()?;

                let caps_filter_glformat = gstreamer::ElementFactory::make("capsfilter")
                    .name("video_glformat_caps")
                    .build()?;

                caps_filter_glformat.set_property(
                    "caps",
                    gstreamer::Caps::builder("video/x-raw")
                        .features(["memory:GLMemory"])
                        .field("format", "NV12")
                        // we capture unlimited, but then downstream set a limit
                        .field("framerate", gstreamer::Fraction::new(fps as i32, 1))
                        .build(),
                );

                (
                    vec![
                        video_pipewiresrc.clone(),
                        caps_filter_pipewiresrc,
                        video_watchdog.clone(),
                        video_queue_1.clone(),
                        glupload,
                        caps_filter_glupload,
                        glcolorscale,
                        caps_filter_glscale,
                        glcolorconvert,
                        videorate,
                    ],
                    caps_filter_glformat,
                )
            }
            CodecType::VAAPI => {
                let caps_filter_pipewiresrc = gstreamer::ElementFactory::make("capsfilter")
                    .name("video_pipewiresrc_caps")
                    .build()?;

                let dma_caps = gstreamer::Caps::builder("video/x-raw")
                    .features(["memory:DMABuf"])
                    .build();

                caps_filter_pipewiresrc.set_property("caps", dma_caps);

                let vapostproc = gstreamer::ElementFactory::make("vapostproc")
                    .name("video_vapostproc")
                    .build()?;

                let caps_filter_vaapi = gstreamer::ElementFactory::make("capsfilter")
                    .name("video_vaapi_caps")
                    .build()?;

                let va_nv12_caps = gstreamer::Caps::builder("video/x-raw")
                    .features(["memory:VAMemory"])
                    .field("format", "NV12")
                    .field("width", resolution.0 as i32)
                    .field("height", resolution.1 as i32)
                    .build();

                caps_filter_vaapi.set_property("caps", va_nv12_caps);

                (
                    vec![
                        video_pipewiresrc.clone(),
                        caps_filter_pipewiresrc,
                        video_watchdog.clone(),
                        video_queue_1.clone(),
                        vapostproc,
                    ],
                    caps_filter_vaapi,
                )
            }
            CodecType::Software => {
                let caps_filter_pipewiresrc = gstreamer::ElementFactory::make("capsfilter")
                    .name("video_pipewiresrc_caps")
                    .build()?;

                caps_filter_pipewiresrc
                    .set_property("caps", gstreamer::Caps::builder("video/x-raw").build());

                let videoconvert = gstreamer::ElementFactory::make("videoconvert")
                    .name("video_convert")
                    .build()?;

                let videoscale = gstreamer::ElementFactory::make("videoscale")
                    .name("video_scale")
                    .build()?;

                let caps_filter_videoscale = gstreamer::ElementFactory::make("capsfilter")
                    .name("caps_filter_videoscale")
                    .build()?;

                let videoscale_caps = gstreamer::Caps::builder("video/x-raw")
                    .field("width", resolution.0 as i32)
                    .field("height", resolution.1 as i32)
                    .field("format", "I420")
                    .build();

                caps_filter_videoscale.set_property("caps", videoscale_caps);

                let videorate = gstreamer::ElementFactory::make("videorate")
                    .name("video_rate")
                    .build()?;

                let caps_filter_videorate = gstreamer::ElementFactory::make("capsfilter")
                    .name("caps_filter_videorate")
                    .build()?;

                let videorate_caps = gstreamer::Caps::builder("video/x-raw")
                    .field("framerate", gstreamer::Fraction::new(fps as i32, 1))
                    .build();

                caps_filter_videorate.set_property("caps", videorate_caps);

                (
                    vec![
                        video_pipewiresrc.clone(),
                        caps_filter_pipewiresrc,
                        video_watchdog.clone(),
                        video_queue_1.clone(),
                        videoconvert,
                        videoscale,
                        caps_filter_videoscale,
                        videorate,
                    ],
                    caps_filter_videorate,
                )
            }
        };

        let encoder_pipeline: Vec<gstreamer::Element> = match codec.get_backend() {
            CodecType::NVIDIA => {
                let encoder = gstreamer::ElementFactory::make(codec.get_encoder())
                    .property("bitrate", video_bitrate_kbps as u32)
                    .property("gop-size", DEFAULT_GOP_SIZE)
                    .property_from_str("rc-mode", "cbr")
                    .name("video_encoder")
                    .build()?;

                let mut vector = vec![encoder, parser];
                if let Some(cf) = parser_annexb_caps_filter {
                    vector.push(cf);
                };
                vector
            }
            CodecType::VAAPI => {
                let encoder = gstreamer::ElementFactory::make(codec.get_encoder())
                    .property("bitrate", video_bitrate_kbps as u32)
                    .property("key-int-max", DEFAULT_KEYFRAME_PERIOD)
                    .name("video_encoder")
                    .build()?;

                let mut vector = vec![encoder, parser];
                if let Some(cf) = parser_annexb_caps_filter {
                    vector.push(cf);
                };
                vector
            }
            CodecType::Software => {
                let threads = std::thread::available_parallelism()
                    .map(|n| n.get() as u32)
                    .unwrap_or(4);
                let encoder = match codec {
                    VideoCodec::H264(_) => gstreamer::ElementFactory::make(codec.get_encoder())
                        .property("bitrate", video_bitrate_kbps as u32)
                        .property("key-int-max", DEFAULT_KEYFRAME_PERIOD)
                        .property_from_str("speed-preset", "ultrafast")
                        .property_from_str("tune", "zerolatency")
                        .property("threads", threads)
                        .property("sliced-threads", true)
                        .name("video_h264_software_encoder")
                        .build()?,
                    VideoCodec::H265(_) => gstreamer::ElementFactory::make(codec.get_encoder())
                        .property("bitrate", video_bitrate_kbps as u32)
                        .property("key-int-max", DEFAULT_KEYFRAME_PERIOD)
                        .property_from_str("speed-preset", "ultrafast")
                        .name("video_h265_software_encoder")
                        .build()?,
                    // AV1 has to be really special and use target-bitrate instead..
                    VideoCodec::AV1(_) => gstreamer::ElementFactory::make(codec.get_encoder())
                        .property("target-bitrate", video_bitrate_kbps as u32)
                        .name("video_av1_software_encoder")
                        .build()?,
                };

                let mut vector = vec![encoder, parser];
                if let Some(cf) = parser_annexb_caps_filter {
                    vector.push(cf);
                };
                vector
            }
        };

        let queue_after_encoder = gstreamer::ElementFactory::make("queue")
            .name("queue_after_video_encoder")
            .build()?;

        video_pipeline.push(pre_encode_caps_filter);
        video_pipeline.push(video_queue_2);
        video_pipeline.extend(encoder_pipeline);
        video_pipeline.push(queue_after_encoder.clone());

        for element in video_pipeline.iter() {
            pipeline.add(element)?
        }

        gstreamer::Element::link_many(
            video_pipeline
                .iter()
                .collect::<Vec<&gstreamer::Element>>()
                .as_slice(),
        )?;

        let video_appsink = gstreamer_app::AppSink::builder()
            .name("video_appsink")
            .property("drop", DEFAULT_APPSINK_DROP)
            .property("max-buffers", DEFAULT_APPSINK_MAX_BUFFERS)
            .property("sync", DEFAULT_APPSINK_SYNC)
            .build();

        pipeline.add(video_appsink.upcast_ref::<gstreamer::Element>())?;
        video_pipeline
            .last()
            .ok_or_else(|| WayclipError::Video("No last video element".into()))?
            .link(video_appsink.upcast_ref::<gstreamer::Element>())?;

        self.set_appsink_callbacks(&video_appsink, ContentType::Video)?;

        Ok(())
    }
}
