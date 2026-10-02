use crate::common::gst::{
    GStreamer, caps::GStreamerCapsType, element::GStreamerElementType, pipeline::GStreamerPipeline,
};
use gstreamer::{
    ClockTime, PadProbeData, PadProbeReturn, PadProbeType,
    event::Eos,
    prelude::{ElementExt, ElementExtManual, PadExt, PadExtManual},
};
use std::{
    fs::create_dir_all,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use wayclip_core::{app::clips::ffmpeg::PreviewGenerator, models::error::WayclipError};

pub const DEFAULT_PREVIEW_WIDTH: i32 = 480;
pub const DEFAULT_PREVIEW_BITRATE: u32 = 800;
pub const DEFAULT_PREVIEW_CLIP_LENGTH: u64 = 5;

pub struct PreviewManager;

impl PreviewManager {
    /// This method is responsible for accepting an input path -- the original path of the video
    /// file -- as well as the output path that user wants to write final preview to. The preview
    /// will contain a low-bitrate and low-res 5 second version of the original video.
    ///
    /// To keep this method re-usable, we will accept an input path instead of direct bytes/buffers
    /// of data streams
    pub fn generate_preview(input_path: &Path, output_path: &Path) -> Result<(), WayclipError> {
        // Make sure the parent directories exist
        if let Some(parent) = output_path.parent() {
            create_dir_all(parent)?;
        }
        // Make sure the original file actually exists
        if !input_path.exists() {
            return Err(WayclipError::Remux("Preview file does not exist".into()));
        }

        // Initialise our custom pipeline object
        let pipeline = GStreamerPipeline::new();

        // filesrc -> decodebin | Will read from disk and auto-detect the correct container to
        // stream the data dynamically
        // video_convert_1 & video_convert_2 | Conver the color space before and after scaling
        // video_scale | Scales the video to the lower resolution
        // x264enc -> h264parse -> matroskamux -> filesink

        // FileSource will point to the location of our original clip
        let file_src = GStreamer::build_element(GStreamerElementType::FileSource {
            location: input_path.to_path_buf(),
        })?;

        // dynamically stream data & convert color space (#1)
        let decode_bin = GStreamer::build_element(GStreamerElementType::DecodeBin)?;
        let video_convert_1 = GStreamer::build_element(GStreamerElementType::VideoConvert)?;

        // scale the video & convert color space (#2)
        let video_scale = GStreamer::build_element(GStreamerElementType::VideoScale)?;
        let video_convert_2 = GStreamer::build_element(GStreamerElementType::VideoConvert)?;

        // Create a new caps filter to accept and convert only video/x-raw frames with a specific width
        let caps = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
            height: None,
            framerate: None,
            format: None,
            width: Some(DEFAULT_PREVIEW_WIDTH),
            memory: None,
        });
        let caps_filter = GStreamer::build_element(GStreamerElementType::CapsFilter { caps })?;

        // The X264 encoder and H264 parser with fast presets translate and format vidoe to correct
        // tpye
        let encoder = GStreamer::build_element(GStreamerElementType::X264Enc)?;
        let parser = GStreamer::build_element(GStreamerElementType::H264Parse)?;

        // Multiplex and write to disk
        let mux = GStreamer::build_element(GStreamerElementType::MatroskaMux)?;
        let file_sink = GStreamer::build_element(GStreamerElementType::FileSink {
            location: output_path.to_path_buf(),
        })?;

        // 1. Add all elements to the pipeline
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

        // 2. Link upstream: file_src -> decode_bin
        pipeline.link(&file_src, &decode_bin)?;

        // 3. Link downstream processing chain: video_convert_1 -> ... -> parser
        pipeline.link(&video_convert_1, &video_scale)?;
        pipeline.link(&video_scale, &video_convert_2)?;
        pipeline.link(&video_convert_2, &caps_filter)?;
        pipeline.link(&caps_filter, &encoder)?;
        pipeline.link(&encoder, &parser)?;

        // 4. Link decode_bin to video_convert_1 dynamically
        GStreamer::link_dynamic_pad(&decode_bin, video_convert_1, "video/");

        // Acquire pads for probing and muxing
        let mux_sink_pad = mux.request_pad_simple("video_%u").ok_or_else(|| {
            WayclipError::Remux("Failed to request video pad from matroskamux".into())
        })?;
        let parser_pad = parser
            .static_pad("src")
            .ok_or_else(|| WayclipError::Remux("No static src pad".into()))?;

        let eos_sent = Arc::new(AtomicBool::new(false));
        let eos_sent_clone = eos_sent.clone();

        let first_pts: Arc<std::sync::Mutex<Option<ClockTime>>> =
            Arc::new(std::sync::Mutex::new(None));
        let first_pts_clone = first_pts.clone();

        // Pad probing to enforce 5-second preview length
        parser_pad.add_probe(PadProbeType::BUFFER, move |pad, info| {
            if eos_sent_clone.load(Ordering::SeqCst) {
                return PadProbeReturn::Drop;
            }

            if let Some(PadProbeData::Buffer(ref buffer)) = info.data {
                let pts = buffer.pts().unwrap_or(ClockTime::ZERO);
                let mut first = first_pts_clone.lock().unwrap();
                let start_pts = *first.get_or_insert(pts);

                let elapsed = pts.saturating_sub(start_pts);

                if elapsed >= ClockTime::from_seconds(DEFAULT_PREVIEW_CLIP_LENGTH) {
                    eos_sent_clone.store(true, Ordering::SeqCst);
                    pad.push_event(Eos::new());
                    return PadProbeReturn::Drop;
                }
            }

            PadProbeReturn::Ok
        });

        // 5. Link parser -> mux -> file_sink
        parser_pad.link(&mux_sink_pad)?;
        pipeline.link(&mux, &file_sink)?;

        // Play and wait for the pipeline to play out
        pipeline.play_and_wait_eos(ClockTime::from_seconds(10), ClockTime::from_seconds(30))?;

        // Preview are hardcoded MKV files
        log::debug!(
            "Finished saving MKV preview to {}",
            output_path.to_string_lossy()
        );

        Ok(())
    }
}

/// To avoid deadlocks
impl PreviewGenerator for PreviewManager {
    fn generate_preview(&self, video_path: &Path, preview_path: &Path) -> Result<(), WayclipError> {
        Self::generate_preview(video_path, preview_path)
    }
}
