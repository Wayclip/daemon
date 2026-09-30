use crate::common::{
    gst::{
        GStreamer,
        app::{EncodedFrameInfo, GStreamerApp},
        element::GStreamerElementType,
        pipeline::GStreamerPipeline,
    },
    video::ring::data::{ContentType, SaveData},
};
use gstreamer::{
    ClockTime,
    glib::object::Cast,
    prelude::{ElementExt, ElementExtManual, PadExt},
};
use gstreamer_app::AppSrc;
use std::{fs::create_dir_all, path::Path};
use wayclip_core::{
    models::error::WayclipError,
    settings::{
        output::VideoFormat,
        recording::{AudioCodec, CodecType, VideoCodec},
    },
};

// We limit the saving to be 120 seconds long...
const DEFAULT_VIDEO_SAVE_TIMEOUT: u64 = 120;

pub mod preview;
pub mod ring;

/// DataStream represents the type of data streams that are comming into our Remux system
/// Each of the valid streams (video/audio) contains a parser field (pulled from settings)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataStream {
    Video(String),
    Audio(String),
}

impl DataStream {
    /// Parse the structure (caps) to extract the correct DataStream type
    pub fn from_structure(structure: &gstreamer::StructureRef) -> Result<Self, WayclipError> {
        Ok(match structure.name().as_str() {
            // Video Cases
            name if name.starts_with("video/x-h264") => Self::Video(
                VideoCodec::H264(CodecType::Software)
                    .get_parser()
                    .to_string(),
            ),
            name if name.starts_with("video/x-h265") => Self::Video(
                VideoCodec::H265(CodecType::Software)
                    .get_parser()
                    .to_string(),
            ),
            name if name.starts_with("video/x-av1") => Self::Video(
                VideoCodec::AV1(CodecType::Software)
                    .get_parser()
                    .to_string(),
            ),
            // Audio cases
            name if name.starts_with("audio/x-opus") => {
                Self::Audio(AudioCodec::Opus.get_parser().to_string())
            }
            // We handle both MP4 (v1) and AAC (v2/v4)
            name if name.starts_with("audio/mpeg") => {
                let codec = match structure.get::<i32>("mpegversion").unwrap_or(1) {
                    2 | 4 => AudioCodec::AAC,
                    _ => AudioCodec::MP3,
                };
                Self::Audio(codec.get_parser().to_string())
            }
            _ => {
                return Err(WayclipError::Remux(
                    format!("Unrecognised structure: {}", structure.name()).into(),
                ));
            }
        })
    }

    /// Method to extract the needed pad template based off the video format. MPEGTS uses sinks,
    /// whereas everyone else doesnt
    pub fn pad_template(&self, format: &VideoFormat) -> &str {
        match format {
            VideoFormat::MPEGTS => "sink_%d",
            _ => match self {
                DataStream::Video { .. } => "video_%u",
                DataStream::Audio { .. } => "audio_%u",
            },
        }
    }

    // as_str since into_inner would imply consuming the value
    pub fn as_str(&self) -> &str {
        match self {
            DataStream::Video(parser) | DataStream::Audio(parser) => parser,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SaveManager;

impl SaveManager {
    pub fn save_clip(
        save_data: SaveData,
        video_format: VideoFormat,
        output_path: &Path,
    ) -> Result<(), WayclipError> {
        log::debug!(
            "Starting save pipeline. {} video frames, {} audio frames => {}",
            save_data.video_frames.len(),
            save_data.audio_frames.len(),
            output_path.to_string_lossy()
        );

        // Validation checks
        // Make suure we actually have video frames. (Audio frames missing is not critical)
        // Also make sure the parent dir is fully created
        if save_data.video_frames.is_empty() {
            return Err(WayclipError::Remux("No video frames were captured!".into()));
        }
        if let Some(parent) = output_path.parent() {
            create_dir_all(parent)?;
        }

        // Intialise our new pipeline object
        let pipeline = GStreamerPipeline::new();

        // Extract the base pts & offset - we will use these as anchor points to make sure video,
        // audio and the real timestamps are synced properly
        let (video_base_pts, audio_base_pts) = save_data.get_base_pts();
        let sync_offset_ms = save_data.sync_offset_ms;

        // -- Structures & Streams --
        // For every content type, there is a data structure. We have to extract this data structure
        // to get the entire data stream. Using this data stream we can then build the app_src
        // element
        //
        // It also worthy to mention that we build our audio_setup here, to make suure we own the
        // data and there are no partial moves being done later on

        let video_structure = save_data.get_video_structure()?;
        let video_stream = DataStream::from_structure(video_structure)?;

        let audio_setup = if let Some((caps, structure)) = save_data.get_audio_structure()? {
            let audio_stream = DataStream::from_structure(structure)?;
            let audio_appsrc = GStreamerApp::build_app_src(&caps);
            Some((audio_appsrc, audio_stream))
        } else {
            None
        };

        // Cleaning the video caps is essential to remove any features from the captured video caps,
        // making it safer to work with the capabilities
        let clean_video_caps = save_data.clear_caps(save_data.video_caps.clone());

        // Now for each, we can extract the collected frames
        let video_frames = save_data.video_frames;
        let audio_frames = save_data.audio_frames;

        // -- Pipeline Creation --
        // This pipeline will be responsible for saving the data from our frames into a local disk
        // file. The following is the outline of our pipeline:
        // mux -> file_sink
        // video_appsrc -> video_parser -> video_mux_pad

        // The mux is basically the container that will hold the data.
        // Since user may switch between different formats, we have to dynamically get the mux
        // element. In contrast, the preview pipeline and the daemon itself, internally, all use the
        // matroskamux muxer.
        let mux = GStreamer::build_element(GStreamerElementType::SaveMux {
            mux: video_format.get_mux().into(),
        })?;

        let file_sink = GStreamer::build_element(GStreamerElementType::FileSink {
            location: output_path.to_path_buf(),
        })?;

        pipeline.add_many([&mux, &file_sink])?;
        mux.link(&file_sink)?;

        // The AppSrc elelemnt will feed our raw memory buffers/frames into gstreamer, which is what
        // actually allows to save the clip onto the disk instead of keeping it in memory
        let video_appsrc = GStreamerApp::build_app_src(&clean_video_caps);

        // dynamic parser (rec. h264parser)
        let video_parser = GStreamer::build_element(GStreamerElementType::SaveVideoParser {
            parser: video_stream.as_str().to_string().into(),
        })?;

        pipeline.add_many([
            video_appsrc.upcast_ref::<gstreamer::Element>(),
            &video_parser,
        ])?;
        video_appsrc
            .upcast_ref::<gstreamer::Element>()
            .link(&video_parser)?;

        // TODO: DYNAMIC PADS
        let video_mux_pad = mux
            .request_pad_simple(video_stream.pad_template(&video_format))
            .ok_or_else(|| WayclipError::Remux("Couldnt request video pad from muxer".into()))?;

        video_parser
            .static_pad("src")
            .ok_or_else(|| WayclipError::Remux("No static src pad".into()))?
            .link(&video_mux_pad)?;

        // We can create a vector of stream data, so we can add streams before to it based off
        // conditions, before actually playing anything. Although might be worthy to consider if
        // this will increase our memory usage, since now we have to hold both of them in memory
        // instead of directly feeding into their respective AppSrc
        let mut streams: Vec<(AppSrc, EncodedFrameInfo)> = Vec::new();

        streams.push((
            video_appsrc,
            EncodedFrameInfo {
                frames: video_frames,
                base_pts: video_base_pts,
                offset_ms: 0,
                content_type: ContentType::Video,
            },
        ));

        // If audio was detected from our previous setup, we have to add it to our pipeline as well
        if let Some((audio_appsrc, audio_stream)) = audio_setup {
            log::info!("Audio enabled");
            let audio_parser = GStreamer::build_element(GStreamerElementType::SaveAudioParser {
                parser: audio_stream.as_str().to_string().into(),
            })?;

            pipeline.add_many([
                audio_appsrc.upcast_ref::<gstreamer::Element>(),
                &audio_parser,
            ])?;
            audio_appsrc
                .upcast_ref::<gstreamer::Element>()
                .link(&audio_parser)?;

            // TODO: DYNAMIC PADS
            let audio_mux_pad = mux
                .request_pad_simple(audio_stream.pad_template(&video_format))
                .ok_or_else(|| {
                    WayclipError::Remux("Couldnt request audio pad from muxer".into())
                })?;
            audio_parser
                .static_pad("src")
                .ok_or_else(|| WayclipError::Remux("No static src pad".into()))?
                .link(&audio_mux_pad)?;

            streams.push((
                audio_appsrc,
                EncodedFrameInfo {
                    frames: audio_frames,
                    base_pts: audio_base_pts,
                    offset_ms: sync_offset_ms,
                    content_type: ContentType::Audio,
                },
            ));
        }

        // Then, set the state to playing, push the frames into the AppSrc & wait until EOS
        pipeline.set_state(gstreamer::State::Playing)?;

        let mut handles = Vec::new();
        for stream in streams {
            handles.push(GStreamerApp::push_frames(stream.0, stream.1));
        }

        for handle in handles {
            handle
                .join()
                .map_err(|_| WayclipError::Remux("Frame pushing thread panicked".into()))??;
        }

        pipeline.wait_eos(ClockTime::from_seconds(DEFAULT_VIDEO_SAVE_TIMEOUT))
    }
}
