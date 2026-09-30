use gstreamer::Element;
use gstreamer::{State, glib::object::Cast};
use wayclip_core::models::error::WayclipError;
use wayclip_core::settings::UserSettings;

use crate::common::gst::GStreamer;
use crate::common::gst::app::{DEFAULT_APPSRC_DO_TIMESTAMP, GStreamerApp};
use crate::common::gst::caps::{GStreamerCapsType, VideoXRawFormat, VideoXRawMemory};
use crate::common::gst::element::{
    GStreamerElement, GStreamerElementProperty, GStreamerElementPropertyValue, GStreamerElementType,
};
use crate::linux::core::engine::pipewire::DaemonEngineConnectionData;
use crate::linux::core1::DEFAULT_PIPEWIRE_DO_TIMESTAMP;
use gstreamer::prelude::{ElementExt, PadExtManual};
use wayclip_core::settings::recording::{CodecType, VideoCodec};

const DEFAULT_CONFIG_INTERVAL: i32 = 1;
const DEFAULT_GST_LEAKY_DOWNSTREAM: &str = "2";
const DEFAULT_MAX_SIZE_BUFFER: u32 = 2;
const DEFAULT_MAX_SIZE_BYTES: u32 = 0;
const DEFAULT_MAX_SIZE_TIME_NS: u64 = 0;
const DEFAULT_GOP_SIZE: i32 = 30;
const DEFAULT_KEYFRAME_PERIOD: u32 = 30;

use crate::common::gst::pipeline::GStreamerPipeline;

pub type ParserFilter = (gstreamer::Element, Option<gstreamer::Element>);

// GStreamer related actions
pub struct DaemonEngineGStreamer {
    pub pipeline: GStreamerPipeline,
}

impl DaemonEngineGStreamer {
    pub fn new() -> Result<Self, WayclipError> {
        // The GStreamerPipeline will already initialise gstreamer::init(), so we can safely call to
        // get the GLDisplayEGL
        let pipeline = GStreamerPipeline::new();
        Ok(Self { pipeline: pipeline })
    }

    pub fn stop(&mut self) -> Result<(), WayclipError> {
        // Setting state to Null already tears down the whole pipeline
        self.pipeline.set_state(State::Null)?;
        Ok(())
    }

    pub fn setup_gstreamer(
        &mut self,
        user_settings: &UserSettings,
        connection_data: &DaemonEngineConnectionData,
    ) -> Result<(), WayclipError> {
        if matches!(
            user_settings.recording.video.codec.get_backend(),
            CodecType::NVIDIA
        ) {
            let gl_display =
                gstreamer_gl_egl::GLDisplayEGL::new()?.upcast::<gstreamer_gl::GLDisplay>();
            self.pipeline.bind_gl_display(&gl_display)?;
        }

        self.setup_video(user_settings, connection_data)?;

        //    let mix = daemon.build_mix_pipeline(&pipeline, &config.audio)?;

        //    let sample_rate = &config.audio.sample_rate_hz;
        //    daemon.build_audio_source_pipeline(
        //        &pipeline,
        //        &config.audio.microphone,
        //        DefaultDeviceType::Microphone,
        //        sample_rate.0,
        //        &mix,
        //    )?;
        //    daemon.build_audio_source_pipeline(
        //        &pipeline,
        //        &config.audio.background,
        //        DefaultDeviceType::Background,
        //        sample_rate.0,
        //        &mix,
        //    )?;

        //    // Update all the 2 billion states
        //    pipeline.set_start_time(ClockTime::ZERO);
        //    pipeline.set_base_time(ClockTime::ZERO);

        //    if let Err(e) = pipeline.set_state(gstreamer::State::Playing) {
        //        let bus = pipeline
        //            .bus()
        //            .ok_or_else(|| WayclipError::Validation("No bus found".into()))?;
        //        let mut reason = "unknown".to_string();
        //        while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::from_mseconds(500)) {
        //            if let gstreamer::MessageView::Error(err) = msg.view() {
        //                reason = format!(
        //                    "{} ({:?}) from element {:?}",
        //                    err.error(),
        //                    err.debug(),
        //                    err.src().map(|s| s.to_string())
        //                );
        //                break;
        //            }
        //        }
        //        let _ = pipeline.set_state(gstreamer::State::Null);
        //        daemon.status = DaemonStatus::Failed;
        //        return Err(WayclipError::Validation(
        //            format!("set_state(Playing) failed synchronously ({e:?}): {reason}").into(),
        //        ));
        //    }
        //}
        //let pipeline_for_wait = pipeline.clone();
        //let (state_result, current_state, _pending) = tokio::task::spawn_blocking(move || {
        //    pipeline_for_wait.state(gstreamer::ClockTime::from_seconds(10))
        //})
        //.await
        //.map_err(|e| WayclipError::Validation(e.to_string().into()))?;

        //if state_result.is_err() || current_state != gstreamer::State::Playing {
        //    // get the error
        //    let bus = pipeline
        //        .bus()
        //        .ok_or_else(|| WayclipError::Validation("No bus found".into()))?;
        //    let mut reason = "unknown".to_string();
        //    // this whole thing is same as for preview/saving
        //    while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::ZERO) {
        //        if let gstreamer::MessageView::Error(e) = msg.view() {
        //            reason = format!("{} ({:?})", e.error(), e.debug());
        //            break;
        //        }
        //    }

        //    let _ = pipeline.set_state(gstreamer::State::Null);
        //    let mut daemon = daemon_arc.lock().await;
        //    daemon.status = DaemonStatus::Failed;
        //    return Err(WayclipError::Validation(
        //        format!(
        //            "Capture pipeline failed to reach PLAYING state ({:?}): {reason}",
        //            state_result
        //        )
        //        .into(),
        //    ));
        //}

        //Ok(pipeline)
        Ok(())
    }

    fn setup_video(
        &mut self,
        user_settings: &UserSettings,
        connection_data: &DaemonEngineConnectionData,
    ) -> Result<(), WayclipError> {
        let (file_descriptor, node_id) = connection_data.extract_data()?;

        let pipewire_src = GStreamer::build_element(GStreamerElementType::VideoPipewireSrc {
            do_timestamp: DEFAULT_APPSRC_DO_TIMESTAMP,
            fd: file_descriptor,
            path: node_id.into(),
            keepalive_ms: 1000 / user_settings.recording.video.fps.0 as i32,
        })?;

        #[cfg(debug_assertions)]
        if let Some(src_pad) = pipewire_src.static_pad("src") {
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

        let video_queue_1 = GStreamer::build_element(GStreamerElementType::VideoQueue {
            buffers: DEFAULT_MAX_SIZE_BUFFER,
            bytes: DEFAULT_MAX_SIZE_BYTES,
            time: DEFAULT_MAX_SIZE_TIME_NS,
            leaky: DEFAULT_GST_LEAKY_DOWNSTREAM.into(),
        })?;

        let video_queue_2 = GStreamer::build_element(GStreamerElementType::VideoQueue {
            buffers: DEFAULT_MAX_SIZE_BUFFER,
            bytes: DEFAULT_MAX_SIZE_BYTES,
            time: DEFAULT_MAX_SIZE_TIME_NS,
            leaky: DEFAULT_GST_LEAKY_DOWNSTREAM.into(),
        })?;

        let (parser, parser_caps_filter) =
            self.get_parser_and_filter(&user_settings.recording.video.codec)?;

        let (mut video_pipeline, pre_encode_caps_filter) =
            self.get_parser_pipeline(pipewire_src, video_queue_1, user_settings)?;

        let encoder_pipeline =
            self.get_encoder_pipeline(user_settings, parser, parser_caps_filter)?;

        // Although this is not really an audio queue, i just define audio queue as being queue with
        // no parameters :)
        // TODO: CHANGE
        let video_queue_3 = GStreamer::build_element(GStreamerElementType::AudioQueue)?;

        video_pipeline.push(pre_encode_caps_filter);
        video_pipeline.push(video_queue_2);
        video_pipeline.extend(encoder_pipeline);
        video_pipeline.push(video_queue_3.clone());

        self.pipeline.add_and_link(
            video_pipeline
                .iter()
                .collect::<Vec<&gstreamer::Element>>()
                .as_slice(),
        )?;

        let video_appsink = GStreamerApp::build_app_sink();
        let video_appsink_ref = video_appsink.upcast_ref::<gstreamer::Element>();

        let last = self
            .pipeline
            .last_element()
            .ok_or_else(|| WayclipError::Video("No last video element".into()))?;
        self.pipeline.add(video_appsink_ref)?;

        self.pipeline.link(&last, video_appsink_ref)?;

        // TODO:
        //self.set_appsink_callbacks(&video_appsink, ContentType::Video)?;

        Ok(())
    }

    fn get_parser_and_filter(&self, codec: &VideoCodec) -> Result<ParserFilter, WayclipError> {
        match codec {
            VideoCodec::H264(_) => {
                let parser = GStreamerElement {
                    factoryname: codec.get_parser(),
                    properties: vec![GStreamerElementProperty {
                        name: "config-interval".into(),
                        value: GStreamerElementPropertyValue::Typed(DEFAULT_CONFIG_INTERVAL.into()),
                    }],
                }
                .build_element()?;

                let caps = GStreamer::build_caps(GStreamerCapsType::VideoXH264);
                let caps_filter =
                    GStreamer::build_element(GStreamerElementType::CapsFilter { caps })?;

                Ok((parser, Some(caps_filter)))
            }
            VideoCodec::H265(_) => {
                let parser = GStreamerElement {
                    factoryname: codec.get_parser(),
                    properties: vec![GStreamerElementProperty {
                        name: "config-interval".into(),
                        value: GStreamerElementPropertyValue::Typed(DEFAULT_CONFIG_INTERVAL.into()),
                    }],
                }
                .build_element()?;

                let caps = GStreamer::build_caps(GStreamerCapsType::VideoXH265);
                let caps_filter =
                    GStreamer::build_element(GStreamerElementType::CapsFilter { caps })?;
                Ok((parser, Some(caps_filter)))
            }
            VideoCodec::AV1(_) => {
                let parser = GStreamerElement {
                    factoryname: codec.get_parser(),
                    ..Default::default()
                }
                .build_element()?;

                Ok((parser, None))
            }
        }
    }

    fn get_parser_pipeline(
        &self,
        pipewire_src: Element,
        video_queue_1: Element,
        user_settings: &UserSettings,
    ) -> Result<(Vec<Element>, Element), WayclipError> {
        match user_settings.recording.video.codec.get_backend() {
            CodecType::NVIDIA => {
                // for nvidia we have DMABuf -> glupload -> GLMemory -> format + colorconvert ->
                // GLMemory NV12
                // First capture DMA
                let caps_1 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
                    width: None,
                    height: None,
                    framerate: None,
                    format: None,
                    memory: Some(VideoXRawMemory::DMABuf),
                });
                let caps_filter_1 =
                    GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_1 })?;

                // Upload DMABuf to GL
                let gl_upload = GStreamer::build_element(GStreamerElementType::GLUpload)?;

                // Then make sure everyuthing is GLMemory
                let caps_2 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
                    width: None,
                    height: None,
                    framerate: None,
                    format: None,
                    memory: Some(VideoXRawMemory::GLMemory),
                });
                let caps_filter_2 =
                    GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_2 })?;

                // GL processing
                let gl_color_convert_pre =
                    GStreamer::build_element(GStreamerElementType::GLColorConvert)?;
                let gl_color_scale = GStreamer::build_element(GStreamerElementType::GLColorScale)?;

                // More filtering
                let caps_3 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
                    width: Some(user_settings.recording.video.resolution.width as i32),
                    height: Some(user_settings.recording.video.resolution.height as i32),
                    framerate: None,
                    format: Some(VideoXRawFormat::RGBA),
                    memory: Some(VideoXRawMemory::GLMemory),
                });
                let caps_filter_3 =
                    GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_3 })?;

                // More GL processingg!
                let gl_color_convert_post =
                    GStreamer::build_element(GStreamerElementType::GLColorConvert)?;

                let videorate = GStreamer::build_element(GStreamerElementType::VideoRate)?;

                let caps_4 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
                    width: None,
                    height: None,
                    framerate: Some(gstreamer::Fraction::new(
                        user_settings.recording.video.fps.0 as i32,
                        1,
                    )),
                    format: Some(VideoXRawFormat::NV12),
                    memory: Some(VideoXRawMemory::GLMemory),
                });
                let caps_filter_4 =
                    GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_4 })?;

                Ok((
                    vec![
                        // pipewire from main pipeline
                        pipewire_src.clone(),
                        caps_filter_1,
                        // queue from main pipeline
                        video_queue_1.clone(),
                        gl_upload,
                        caps_filter_2,
                        gl_color_convert_pre,
                        gl_color_scale,
                        caps_filter_3,
                        gl_color_convert_post,
                        videorate,
                    ],
                    caps_filter_4,
                ))
            }
            CodecType::VAAPI => {
                let caps_1 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
                    width: None,
                    height: None,
                    framerate: None,
                    format: None,
                    memory: Some(VideoXRawMemory::DMABuf),
                });
                let caps_filter_1 =
                    GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_1 })?;

                let vapostproc = GStreamer::build_element(GStreamerElementType::VAPostProc)?;

                let caps_2 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
                    width: Some(user_settings.recording.video.resolution.width as i32),
                    height: Some(user_settings.recording.video.resolution.height as i32),
                    // TODO: VAAPI NO FPS??
                    framerate: None,
                    format: Some(VideoXRawFormat::NV12),
                    memory: Some(VideoXRawMemory::VAMemory),
                });
                let caps_filter_2 =
                    GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_2 })?;

                Ok((
                    vec![
                        // pipewire from main pipeline
                        pipewire_src.clone(),
                        caps_filter_1,
                        // queue from main pipeline
                        video_queue_1.clone(),
                        vapostproc,
                    ],
                    caps_filter_2,
                ))
            }
            CodecType::Software => {
                let caps_1 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
                    width: None,
                    height: None,
                    framerate: None,
                    format: None,
                    memory: None,
                });
                let caps_filter_1 =
                    GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_1 })?;

                let videoconvert = GStreamer::build_element(GStreamerElementType::VideoConvert)?;
                let videoscale = GStreamer::build_element(GStreamerElementType::VideoScale)?;

                let caps_2 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
                    width: Some(user_settings.recording.video.resolution.width as i32),
                    height: Some(user_settings.recording.video.resolution.height as i32),
                    framerate: None,
                    format: Some(VideoXRawFormat::I420),
                    memory: None,
                });
                let caps_filter_2 =
                    GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_2 })?;

                let videorate = GStreamer::build_element(GStreamerElementType::VideoRate)?;

                let caps_3 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
                    width: None,
                    height: None,
                    framerate: Some(gstreamer::Fraction::new(
                        user_settings.recording.video.fps.0 as i32,
                        1,
                    )),
                    format: None,
                    memory: None,
                });
                let caps_filter_3 =
                    GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_3 })?;

                Ok((
                    vec![
                        // pipewire from main pipeline
                        pipewire_src.clone(),
                        caps_filter_1,
                        // queue from main pipeline
                        video_queue_1.clone(),
                        videoconvert,
                        videoscale,
                        caps_filter_2,
                        videorate,
                    ],
                    caps_filter_3,
                ))
            }
        }
    }

    fn get_encoder_pipeline(
        &self,
        user_settings: &UserSettings,
        parser: Element,
        parser_caps_filter: Option<Element>,
    ) -> Result<Vec<Element>, WayclipError> {
        let codec = &user_settings.recording.video.codec;
        let bitrate = user_settings.recording.video.bitrate_kbps.0;

        let mut vector = match codec.get_backend() {
            CodecType::NVIDIA => {
                let encoder = GStreamerElement {
                    factoryname: codec.get_encoder(),
                    properties: vec![
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
                }
                .build_element()?;

                vec![encoder, parser]
            }
            CodecType::VAAPI => {
                let encoder = GStreamerElement {
                    factoryname: codec.get_encoder(),
                    properties: vec![
                        GStreamerElementProperty {
                            name: "bitrate".into(),
                            value: GStreamerElementPropertyValue::Typed(bitrate.into()),
                        },
                        GStreamerElementProperty {
                            name: "key-int-max".into(),
                            value: GStreamerElementPropertyValue::Typed(
                                DEFAULT_KEYFRAME_PERIOD.into(),
                            ),
                        },
                    ],
                }
                .build_element()?;

                vec![encoder, parser]
            }
            CodecType::Software => {
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
                            value: GStreamerElementPropertyValue::Typed(
                                DEFAULT_KEYFRAME_PERIOD.into(),
                            ),
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
                            value: GStreamerElementPropertyValue::Typed(
                                DEFAULT_KEYFRAME_PERIOD.into(),
                            ),
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

                let encoder = GStreamerElement {
                    factoryname: codec.get_encoder(),
                    properties,
                }
                .build_element()?;

                vec![encoder, parser]
            }
        };

        vector.extend(parser_caps_filter);
        Ok(vector)
    }
}
