use std::sync::Mutex;
use wayclip_core::settings::recording::VideoCodec;

pub struct VideoNvidia;

impl VideoNvidia {
    // Check if current system has cuda supported.
    fn cuda_path_supported(codec: &VideoCodec) -> Result<(), String> {
        let _ = gstreamer::init();

        for element in ["cudaupload", "cudaconvertscale"] {
            if gstreamer::ElementFactory::find(element).is_none() {
                return Err(format!("GStreamer element `{element}` is missing"));
            }
        }

        let encoder = codec.get_encoder();
        let cuda_caps = gstreamer::Caps::builder("video/x-raw")
            .features(["memory:CUDAMemory"])
            .build();

        let accepts_cuda = gstreamer::ElementFactory::find(&encoder).is_some_and(|factory| {
            factory.static_pad_templates().iter().any(|template| {
                template.direction() == gstreamer::PadDirection::Sink
                    && template.caps().can_intersect(&cuda_caps)
            })
        });

        if !accepts_cuda {
            return Err(format!("`{encoder}` does not accept memory:CUDAMemory"));
        }

        Ok(())
    }

    /// We cannot switch to only one since systems, versions and driver vary.
    /// `WAYCLIP_NVIDIA_GL=1` forces DMABuf/GL
    /// `WAYCLIP_NVIDIA_CUDA=1` forces CUDA
    pub fn nvidia_cuda_enabled(codec: &VideoCodec) -> bool {
        static LAST: Mutex<Option<(String, bool)>> = Mutex::new(None);

        let encoder = codec.get_encoder().into_owned();
        let (cuda, reason) = if matches!(std::env::var("WAYCLIP_NVIDIA_GL").as_deref(), Ok("1")) {
            (false, "WAYCLIP_NVIDIA_GL=1".to_string())
        } else if matches!(std::env::var("WAYCLIP_NVIDIA_CUDA").as_deref(), Ok("1")) {
            (true, "WAYCLIP_NVIDIA_CUDA=1".to_string())
        } else {
            match Self::cuda_path_supported(codec) {
                Ok(()) => (true, String::new()),
                Err(reason) => (false, reason),
            }
        };

        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_ref() != Some(&(encoder.clone(), cuda)) {
            if cuda {
                log::info!(
                    "NVIDIA capture: CUDA path ({}), no EGL context, no DMABuf zero-copy",
                    if reason.is_empty() {
                        "auto-detected"
                    } else {
                        &reason
                    }
                );
            } else {
                log::info!("NVIDIA capture: OpenGL path, {reason}");
            }
            *last = Some((encoder, cuda));
        }

        cuda
    }
}
