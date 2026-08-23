use crate::common::ring::{ContentType, EncodedFrame, SaveData};
use gstreamer::{
    ClockTime, PadProbeData, PadProbeReturn, PadProbeType,
    event::Eos,
    glib::object::Cast,
    prelude::{ElementExt, ElementExtManual, GstBinExtManual, PadExt, PadExtManual},
};
use gstreamer_app::AppSrc;
use log::{error, info, warn};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use wayclip_core::{
    app::clips::ffmpeg::PreviewGenerator,
    models::error::WayclipError,
    settings::{
        output::VideoFormat,
        recording::{AudioCodec, CodecType, VideoCodec},
    },
};

// this file is basically the pipeline of things that happens after user saves the clip.
// making the preview + saving and draining the ring buffer.
// mostly works cross-platform

const DEFAULT_PREVIEW_WIDTH: i32 = 480;
const DEFAULT_PREVIEW_BITRATE: u32 = 800;
const DEFAULT_PREVIEW_CLIP_LENGTH: u64 = 5;
const DEFAULT_VIDEO_SAVE_TIMEOUT: u64 = 120;
const DEFAULT_PREVIEW_DELAY_MS: u64 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemuxStream {
    Video { parser: String },
    Audio { parser: String },
    Skip,
}

impl RemuxStream {
    pub fn from_caps(structure: &gstreamer::StructureRef) -> Self {
        match structure.name().as_str() {
            name if name.starts_with("video/x-h264") => Self::Video {
                parser: VideoCodec::H264(CodecType::Software)
                    .get_parser()
                    .to_string(),
            },
            name if name.starts_with("video/x-h265") => Self::Video {
                parser: VideoCodec::H265(CodecType::Software)
                    .get_parser()
                    .to_string(),
            },
            name if name.starts_with("video/x-av1") => Self::Video {
                parser: VideoCodec::AV1(CodecType::Software)
                    .get_parser()
                    .to_string(),
            },
            name if name.starts_with("audio/x-opus") => Self::Audio {
                parser: AudioCodec::Opus.get_parser().to_string(),
            },
            // both mp3 (v1) and aac (v2/v4)
            name if name.starts_with("audio/mpeg") => {
                let codec = match structure.get::<i32>("mpegversion").unwrap_or(1) {
                    2 | 4 => AudioCodec::AAC,
                    _ => AudioCodec::MP3,
                };

                Self::Audio {
                    parser: codec.get_parser().to_string(),
                }
            }
            _ => Self::Skip,
        }
    }

    pub fn pad_template(&self, format: &VideoFormat) -> &str {
        match format {
            VideoFormat::MPEGTS => "sink_%d",
            _ => match self {
                RemuxStream::Video { .. } => "video_%u",
                RemuxStream::Audio { .. } => "audio_%u",
                RemuxStream::Skip => unreachable!(),
            },
        }
    }

    pub fn parser_name(&self) -> &str {
        match self {
            RemuxStream::Video { parser } | RemuxStream::Audio { parser } => parser,
            RemuxStream::Skip => unreachable!(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RemuxHandler;

impl RemuxHandler {
    pub fn run_remux_pipeline(
        &self,
        save_data: SaveData,
        video_format: VideoFormat,
        output_path: PathBuf,
    ) -> Result<(), WayclipError> {
        log::debug!(
            "Starting save pipeline: {} video frames, {} audio frames → {}",
            save_data.video_frames.len(),
            save_data.audio_frames.len(),
            output_path.to_string_lossy()
        );

        if save_data.video_frames.is_empty() {
            return Err(WayclipError::Remux("No video frames were captured!".into()));
        }

        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        gstreamer::init()?;

        let video_base_pts = save_data
            .video_frames
            .first()
            .map(|frame| frame.pts)
            .unwrap_or(gstreamer::ClockTime::ZERO);

        let audio_base_pts = save_data
            .audio_frames
            .first()
            .map(|frame| frame.pts)
            .unwrap_or(gstreamer::ClockTime::ZERO);

        let sync_offset_ms = save_data.sync_offset_ms;

        let pipeline = gstreamer::Pipeline::new();

        let video_stream = {
            let structure = save_data
                .video_caps
                .structure(0)
                .ok_or_else(|| WayclipError::Remux("Video caps have no structure".into()))?;
            let stream = RemuxStream::from_caps(structure);
            if stream == RemuxStream::Skip {
                return Err(WayclipError::Remux(
                    format!("Unrecognised video caps: {}", save_data.video_caps).into(),
                ));
            };
            stream
        };

        let mux = gstreamer::ElementFactory::make(video_format.get_mux())
            .name("save_mux")
            .build()?;

        let file_sink = gstreamer::ElementFactory::make("filesink")
            .name("save_filesink")
            .property(
                "location",
                output_path
                    .to_str()
                    .ok_or_else(|| WayclipError::Remux("Output path is not valid UTF-8".into()))?,
            )
            .build()?;

        pipeline.add_many([&mux, &file_sink])?;
        mux.link(&file_sink)?;

        let clean_video_caps = {
            let mut builder = gstreamer::Caps::builder_full();
            for structure in save_data.video_caps.iter() {
                builder = builder.structure(structure.to_owned());
            }
            builder.build()
        };

        let video_appsrc = gstreamer_app::AppSrc::builder()
            .name("video_appsrc")
            .caps(&clean_video_caps)
            .format(gstreamer::Format::Time)
            .is_live(false)
            .do_timestamp(false)
            .build();

        let video_parser = gstreamer::ElementFactory::make(video_stream.parser_name())
            .name("save_video_parser")
            .build()?;

        pipeline.add_many([
            video_appsrc.upcast_ref::<gstreamer::Element>(),
            &video_parser,
        ])?;

        video_appsrc
            .upcast_ref::<gstreamer::Element>()
            .link(&video_parser)?;

        let video_mux_pad = mux
            .request_pad_simple(video_stream.pad_template(&video_format))
            .ok_or_else(|| WayclipError::Remux("Couldnt request video pad from muxer".into()))?;
        video_parser
            .static_pad("src")
            .ok_or_else(|| WayclipError::Remux("No static src pad".into()))?
            .link(&video_mux_pad)?;

        let audio_appsrc_optional: Option<gstreamer_app::AppSrc> =
            match (&save_data.audio_caps, save_data.audio_frames.is_empty()) {
                (Some(caps), false) => {
                    let structure = caps
                        .structure(0)
                        .ok_or_else(|| WayclipError::Remux("No structure".into()))?;
                    let audio_stream = RemuxStream::from_caps(structure);
                    if audio_stream == RemuxStream::Skip {
                        warn!("Unrecognised audio caps {}", caps);
                        None
                    } else {
                        let audio_appsrc = gstreamer_app::AppSrc::builder()
                            .name("audio_appsrc")
                            .caps(caps)
                            .format(gstreamer::Format::Time)
                            .is_live(false)
                            .do_timestamp(false)
                            .build();
                        let audio_parser =
                            gstreamer::ElementFactory::make(audio_stream.parser_name())
                                .name("save_audio_parser")
                                .build()?;

                        pipeline.add_many([
                            audio_appsrc.upcast_ref::<gstreamer::Element>(),
                            &audio_parser,
                        ])?;

                        audio_appsrc
                            .upcast_ref::<gstreamer::Element>()
                            .link(&audio_parser)?;

                        let audio_mux_pad = mux
                            .request_pad_simple(audio_stream.pad_template(&video_format))
                            .ok_or_else(|| {
                                WayclipError::Remux("Couldnt request audio pad from muxer".into())
                            })?;
                        audio_parser
                            .static_pad("src")
                            .ok_or_else(|| WayclipError::Remux("No static src pad".into()))?
                            .link(&audio_mux_pad)?;

                        Some(audio_appsrc)
                    }
                }
                _ => {
                    info!("No audio to include");
                    None
                }
            };

        pipeline.set_state(gstreamer::State::Playing)?;

        let video_frames = save_data.video_frames;
        let video_appsrc_clone = video_appsrc.clone();
        // no offset for this one
        self.push_frames_to_appsrc(
            video_frames,
            video_appsrc_clone,
            video_base_pts,
            0,
            ContentType::Video,
        )?;

        if let Some(audio_appsrc) = audio_appsrc_optional {
            let audio_frames = save_data.audio_frames;
            self.push_frames_to_appsrc(
                audio_frames,
                audio_appsrc,
                audio_base_pts,
                sync_offset_ms,
                ContentType::Audio,
            )?;
        };

        let bus = pipeline
            .bus()
            .ok_or_else(|| WayclipError::Remux("No bus found".into()))?;
        let timeout = gstreamer::ClockTime::from_seconds(DEFAULT_VIDEO_SAVE_TIMEOUT);

        let result = loop {
            match bus.timed_pop(timeout) {
                Some(message) => match message.view() {
                    gstreamer::MessageView::Eos(_) => {
                        log::debug!("Save pipeline completed, EOS");
                        break Ok(());
                    }
                    gstreamer::MessageView::Error(e) => {
                        error!("Save pipeline error: {:?}", e);
                        break Err(WayclipError::Remux(
                            format!("Save pipeline error: {:?}", e).into(),
                        ));
                    }
                    gstreamer::MessageView::Warning(w) => {
                        warn!("Save pipeline warning: {:?}", w);
                    }
                    _ => {}
                },
                None => {
                    break Err(WayclipError::Remux(
                        format!(
                            "Save pipeline timed out after {}s",
                            DEFAULT_VIDEO_SAVE_TIMEOUT
                        )
                        .into(),
                    ));
                }
            }
        };

        pipeline.set_state(gstreamer::State::Null)?;

        if result.is_ok() {
            info!("Clip written to {}", output_path.to_string_lossy())
        }

        result
    }

    fn push_frames_to_appsrc(
        &self,
        frames: Vec<EncodedFrame>,
        appsrc: AppSrc,
        base_pts: ClockTime,
        sync_offset_ms: i64,
        content_type: ContentType,
    ) -> Result<(), WayclipError> {
        std::thread::spawn(move || {
            for frame in frames {
                let mut buffer = frame.data.clone();

                {
                    let buffer_ref = buffer.make_mut();

                    let normalised = frame.pts.checked_sub(base_pts).unwrap_or(ClockTime::ZERO);

                    let pts = if sync_offset_ms >= 0 {
                        normalised.saturating_add(ClockTime::from_mseconds(sync_offset_ms as u64))
                    } else {
                        normalised
                            .saturating_sub(ClockTime::from_mseconds((-sync_offset_ms) as u64))
                    };

                    buffer_ref.set_pts(pts);

                    if let Some(frame_dts) = frame.dts {
                        let dts_norm = frame_dts.checked_sub(base_pts).unwrap_or(ClockTime::ZERO);

                        let dts = if sync_offset_ms >= 0 {
                            dts_norm.saturating_add(ClockTime::from_mseconds(sync_offset_ms as u64))
                        } else {
                            dts_norm
                                .saturating_sub(ClockTime::from_mseconds((-sync_offset_ms) as u64))
                        };

                        buffer_ref.set_dts(dts);
                    }

                    if let Some(duration) = frame.duration {
                        buffer_ref.set_duration(duration);
                    }

                    if !frame.is_keyframe {
                        buffer_ref.set_flags(gstreamer::BufferFlags::DELTA_UNIT);
                    }
                }

                if let Err(e) = appsrc.push_buffer(buffer) {
                    error!("Failed to push {} buffer: {:?}", content_type, e);
                    return;
                }
            }
            if let Err(e) = appsrc.end_of_stream() {
                error!("Failed to send EOS: {:?}", e);
            }
            log::debug!("{} appsrc finished", content_type)
        });
        Ok(())
    }

    pub fn generate_preview(
        &self,
        input_path: PathBuf,
        output_path: PathBuf,
        first_video_pts: Option<ClockTime>,
    ) -> Result<(), WayclipError> {
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::thread::sleep(std::time::Duration::from_millis(DEFAULT_PREVIEW_DELAY_MS));
        if !input_path.exists() {
            return Err(WayclipError::Remux("Preview file does not exist".into()));
        }

        gstreamer::init()?;
        let pipeline = gstreamer::Pipeline::new();

        let file_src = gstreamer::ElementFactory::make("filesrc")
            .property(
                "location",
                input_path
                    .to_str()
                    .ok_or_else(|| WayclipError::Remux("No filesrc location".into()))?,
            )
            .name("file_src")
            .build()?;

        let decode_bin = gstreamer::ElementFactory::make("decodebin")
            .name("decode_bin")
            .build()?;

        let video_convert_1 = gstreamer::ElementFactory::make("videoconvert")
            .name("video_convert_1")
            .build()?;

        let video_scale = gstreamer::ElementFactory::make("videoscale")
            .name("video_scale")
            .build()?;

        let video_convert_2 = gstreamer::ElementFactory::make("videoconvert")
            .name("video_convert_2")
            .build()?;

        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("width", DEFAULT_PREVIEW_WIDTH)
            .build();

        let caps_filter = gstreamer::ElementFactory::make("capsfilter")
            .property("caps", &caps)
            .build()?;

        let encoder = gstreamer::ElementFactory::make("x264enc")
            .property_from_str("tune", "zerolatency")
            .property_from_str("speed-preset", "veryfast")
            .property("bitrate", DEFAULT_PREVIEW_BITRATE)
            .name("x264encoder")
            .build()?;

        let parser = gstreamer::ElementFactory::make("h264parse")
            .name("h264_parser")
            .build()?;

        let mux = gstreamer::ElementFactory::make("matroskamux")
            .name("mux")
            .property("streamable", true)
            .build()?;

        let file_sink = gstreamer::ElementFactory::make("filesink")
            .property(
                "location",
                output_path
                    .to_str()
                    .ok_or_else(|| WayclipError::Remux("Output path is not valid UTF-8".into()))?,
            )
            .name("file_sink")
            .build()?;

        pipeline.add_many([
            &file_src,
            &decode_bin,
            &video_convert_1,
            &video_scale,
            &video_convert_2,
            &caps_filter,
            &encoder,
            &parser,
            &mux,
            &file_sink,
        ])?;

        gstreamer::Element::link_many([
            &video_convert_1,
            &video_scale,
            &video_convert_2,
            &caps_filter,
            &encoder,
            &parser,
        ])?;

        let mux_sink_pad = mux.request_pad_simple("video_%u").ok_or_else(|| {
            WayclipError::Remux("Failed to request video pad from matroskamux".into())
        })?;
        let parser_pad = parser
            .static_pad("src")
            .ok_or_else(|| WayclipError::Remux("No static src pad".into()))?;

        let eos_sent = Arc::new(AtomicBool::new(false));
        let eos_sent_clone = eos_sent.clone();

        let first_video_pts_clone = first_video_pts
            .ok_or_else(|| WayclipError::Remux("Failed to get first video pts".into()))?;

        parser_pad.add_probe(PadProbeType::BUFFER, move |pad, info| {
            if eos_sent_clone.load(Ordering::SeqCst) {
                return PadProbeReturn::Drop;
            }

            // holy new synatx
            if let Some(PadProbeData::Buffer(ref buffer)) = info.data {
                let pts = buffer.pts().unwrap_or(ClockTime::ZERO);
                let elapsed = pts
                    .checked_sub(first_video_pts_clone)
                    .unwrap_or(ClockTime::ZERO);

                if elapsed >= ClockTime::from_seconds(DEFAULT_PREVIEW_CLIP_LENGTH) {
                    log::debug!("Reached preview limit of {DEFAULT_PREVIEW_CLIP_LENGTH}s at {pts}");
                    eos_sent_clone.store(true, Ordering::SeqCst);

                    pad.push_event(Eos::new());
                    return PadProbeReturn::Drop;
                }
            }

            PadProbeReturn::Ok
        });

        parser_pad.link(&mux_sink_pad)?;

        mux.link(&file_sink)?;
        file_src.link(&decode_bin)?;

        let video_convert_1_clone = video_convert_1.clone();
        decode_bin.connect_pad_added(move |_, src_pad| {
            let link_pad = || -> Result<(), WayclipError> {
                let caps = src_pad
                    .current_caps()
                    .ok_or_else(|| WayclipError::Remux("Pad has no current caps".into()))?;

                let structure = caps
                    .structure(0)
                    .ok_or_else(|| WayclipError::Remux("Caps structure is empty".into()))?;

                let name = structure.name();

                if name.starts_with("video/") {
                    let sink_pad = video_convert_1_clone.static_pad("sink").ok_or_else(|| {
                        WayclipError::Remux("Could not find 'sink' pad on videoconvert".into())
                    })?;

                    if !sink_pad.is_linked() {
                        src_pad.link(&sink_pad).map_err(|e| {
                            WayclipError::Remux(
                                format!("Failed to link decodebin to videoconvert: {e}").into(),
                            )
                        })?;
                    }
                }

                Ok(())
            };

            if let Err(e) = link_pad() {
                error!("Error handling new pad in decode_bin: {:?}", e);
            }
        });

        pipeline.set_state(gstreamer::State::Playing)?;
        let (state_result, _, _) = pipeline.state(gstreamer::ClockTime::from_seconds(10));
        if state_result.is_err() {
            let bus = pipeline
                .bus()
                .ok_or_else(|| WayclipError::Remux("No pipeline bus".into()))?;
            let mut reason = "unknown".to_string();
            while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::ZERO) {
                if let gstreamer::MessageView::Error(e) = msg.view() {
                    reason = format!("{} ({:?})", e.error(), e.debug());
                    break;
                }
            }
            pipeline.set_state(gstreamer::State::Null)?;
            return Err(WayclipError::Remux(
                format!("Pipeline failed to reach PLAYING state for preview: {reason}").into(),
            ));
        }

        let bus = pipeline
            .bus()
            .ok_or_else(|| WayclipError::Remux("No pipeline bus".into()))?;

        let timeout = gstreamer::ClockTime::from_seconds(30);
        let result = loop {
            match bus.timed_pop(timeout) {
                Some(message) => match message.view() {
                    gstreamer::MessageView::Eos(_) => break Ok(()),
                    gstreamer::MessageView::Error(e) => {
                        break Err(WayclipError::Remux(
                            format!("Preview error: {:?}", e).into(),
                        ));
                    }
                    _ => {}
                },
                None => break Err(WayclipError::Remux("Preview pipeline timed out".into())),
            }
        };
        pipeline.set_state(gstreamer::State::Null)?;
        result?;

        log::debug!(
            "Finished saving MKV preview to {}",
            output_path.to_string_lossy()
        );

        Ok(())
    }
}

/// To avoid deadlocks
impl PreviewGenerator for RemuxHandler {
    fn generate_preview(&self, video_path: &Path, preview_path: &Path) -> Result<(), WayclipError> {
        self.generate_preview(
            video_path.to_path_buf(),
            preview_path.to_path_buf(),
            Some(ClockTime::ZERO),
        )
    }
}
