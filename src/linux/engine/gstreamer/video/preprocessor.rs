use gstreamer::{Caps, Element};
use std::sync::Arc;
use wayclip_core::{
    models::error::WayclipError,
    settings::{UserSettings, recording::CodecType},
};

use crate::linux::engine::gstreamer::CaptureMode;
use crate::{
    common::gst::{
        GStreamer,
        caps::{GStreamerCapsType, VideoXRawFormat, VideoXRawMemory},
        element::GStreamerElementType,
        guard::{CaptureHealth, VideoBufferGuard},
    },
    linux::engine::gstreamer::video::nvidia::VideoNvidia,
};

pub struct VideoPreprocessFactory;

impl VideoPreprocessFactory {
    fn source_caps(mode: CaptureMode) -> Caps {
        let sys = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
            width: None,
            height: None,
            framerate: None,
            format: None,
            memory: None,
        });
        match mode {
            CaptureMode::SystemMemory => sys,
            CaptureMode::DmaBuf => {
                let mut caps = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
                    width: None,
                    height: None,
                    framerate: None,
                    format: None,
                    memory: Some(VideoXRawMemory::DMABuf),
                });
                caps.merge(sys);
                caps
            }
        }
    }

    pub fn build(
        pipewire_src: Element,
        video_queue: Element,
        user_settings: &UserSettings,
        mode: CaptureMode,
        health: Arc<CaptureHealth>,
    ) -> Result<(Vec<Element>, Element), WayclipError> {
        let codec = &user_settings.recording.video.codec;
        match codec.get_backend() {
            CodecType::NVIDIA => {
                if VideoNvidia::nvidia_cuda_enabled(codec) {
                    Self::build_nvidia_cuda(pipewire_src, video_queue, user_settings, health)
                } else {
                    Self::build_nvidia_pipeline(
                        pipewire_src,
                        video_queue,
                        user_settings,
                        mode,
                        health,
                    )
                }
            }
            CodecType::VAAPI => {
                Self::build_vaapi_pipeline(pipewire_src, video_queue, user_settings, mode, health)
            }
            CodecType::Software => {
                Self::build_software_pipeline(pipewire_src, video_queue, user_settings)
            }
        }
    }

    /// `cudaupload` -> `cudaconvertscale` -> `nvh264enc`
    fn build_nvidia_cuda(
        pipewire_src: Element,
        video_queue: Element,
        user_settings: &UserSettings,
        health: Arc<CaptureHealth>,
    ) -> Result<(Vec<Element>, Element), WayclipError> {
        let caps_1 = Self::source_caps(CaptureMode::SystemMemory);
        let caps_filter_1 =
            GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_1 })?;

        let cuda_upload = GStreamer::build_element(GStreamerElementType::CudaUpload)?;
        VideoBufferGuard::attach(&cuda_upload, health);

        let caps_2 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
            width: None,
            height: None,
            framerate: None,
            format: None,
            memory: Some(VideoXRawMemory::CUDAMemory),
        });
        let caps_filter_2 =
            GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_2 })?;

        let cuda_convert_scale = GStreamer::build_element(GStreamerElementType::CudaConvertScale)?;

        let caps_3 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
            width: Some(user_settings.recording.video.resolution.width as i32),
            height: Some(user_settings.recording.video.resolution.height as i32),
            framerate: None,
            format: Some(VideoXRawFormat::NV12),
            memory: Some(VideoXRawMemory::CUDAMemory),
        });
        let caps_filter_3 =
            GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_3 })?;

        let videorate = GStreamer::build_element(GStreamerElementType::VideoRate)?;
        let caps_4 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
            width: None,
            height: None,
            framerate: Some(gstreamer::Fraction::new(
                user_settings.recording.video.fps.0 as i32,
                1,
            )),
            format: Some(VideoXRawFormat::NV12),
            memory: Some(VideoXRawMemory::CUDAMemory),
        });
        let caps_filter_4 =
            GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_4 })?;

        Ok((
            vec![
                pipewire_src,
                caps_filter_1,
                video_queue,
                cuda_upload,
                caps_filter_2,
                cuda_convert_scale,
                caps_filter_3,
                videorate,
            ],
            caps_filter_4,
        ))
    }

    fn build_nvidia_pipeline(
        pipewire_src: Element,
        video_queue: Element,
        user_settings: &UserSettings,
        mode: CaptureMode,
        health: Arc<CaptureHealth>,
    ) -> Result<(Vec<Element>, Element), WayclipError> {
        // NVIDIA pipeline: (DMABuf | SystemMemory) -> videorate -> glupload (GLMemory) -> glcolorscale -> glcolorconvert -> GLMemory (NV12)
        let caps_1 = Self::source_caps(mode);
        let caps_filter_1 =
            GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_1 })?;

        // Upload to GL
        let gl_upload = GStreamer::build_element(GStreamerElementType::GLUpload)?;
        // drop bad ones
        VideoBufferGuard::attach(&gl_upload, health);

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
        let gl_color_convert_pre = GStreamer::build_element(GStreamerElementType::GLColorConvert)?;
        let gl_color_scale = GStreamer::build_element(GStreamerElementType::GLColorScale)?;

        // More filtering
        let caps_3 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
            width: Some(user_settings.recording.video.resolution.width as i32),
            height: Some(user_settings.recording.video.resolution.height as i32),
            framerate: None,
            format: None,
            memory: Some(VideoXRawMemory::GLMemory),
        });
        let caps_filter_3 =
            GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_3 })?;

        // More GL processingg!
        let gl_color_convert_post = GStreamer::build_element(GStreamerElementType::GLColorConvert)?;

        // Frame rate conversion happens on DMABuf frames, before the GL upload.
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
                video_queue.clone(),
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

    fn build_vaapi_pipeline(
        pipewire_src: Element,
        video_queue: Element,
        user_settings: &UserSettings,
        mode: CaptureMode,
        health: Arc<CaptureHealth>,
    ) -> Result<(Vec<Element>, Element), WayclipError> {
        let caps_1 = Self::source_caps(mode);
        let caps_filter_1 =
            GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_1 })?;

        let vapostproc = GStreamer::build_element(GStreamerElementType::VAPostProc)?;
        VideoBufferGuard::attach(&vapostproc, health);

        let caps_2 = GStreamer::build_caps(GStreamerCapsType::VideoXRaw {
            width: Some(user_settings.recording.video.resolution.width as i32),
            height: Some(user_settings.recording.video.resolution.height as i32),
            framerate: None,
            format: Some(VideoXRawFormat::NV12),
            memory: Some(VideoXRawMemory::VAMemory),
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
            format: Some(VideoXRawFormat::NV12),
            memory: Some(VideoXRawMemory::VAMemory),
        });
        let caps_filter_3 =
            GStreamer::build_element(GStreamerElementType::CapsFilter { caps: caps_3 })?;

        Ok((
            vec![
                // pipewire from main pipeline
                pipewire_src.clone(),
                caps_filter_1,
                // queue from main pipeline
                video_queue.clone(),
                vapostproc,
                caps_filter_2,
                videorate,
            ],
            caps_filter_3,
        ))
    }

    fn build_software_pipeline(
        pipewire_src: Element,
        video_queue: Element,
        user_settings: &UserSettings,
    ) -> Result<(Vec<Element>, Element), WayclipError> {
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
                video_queue.clone(),
                videoconvert,
                videoscale,
                caps_filter_2,
                videorate,
            ],
            caps_filter_3,
        ))
    }
}
