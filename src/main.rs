use std::io::IsTerminal;
use std::io::Write;
use wayclip_daemon::linux::memory::MemoryManager;

fn main() -> std::process::ExitCode {
    MemoryManager::tune_allocator();
    unsafe {
        std::env::set_var("GLOBAL_HOTKEY_APP_ID", "com.wayclip.daemon");
        std::env::set_var("GST_GL_PLATFORM", "egl");
        std::env::set_var("GST_GL_WINDOW", "surfaceless");
    }

    // the driver caps how much an OpenGL process may
    // hold in a reuse pool, and without this NVENC can fail with NV_ENC_ERR_OUT_OF_MEMORY on a
    // card that a game is already using
    if let Err(e) = MemoryManager::reserve_video_memory() {
        log::warn!("Could not apply the NVIDIA video memory profile: {e}");
    }

    match std::io::stderr().is_terminal() {
        true => env_logger::Builder::from_default_env()
            .format(|buf, record| {
                let style = buf.default_level_style(record.level());
                write!(
                    buf,
                    "\r\x1b[2K[{} {style}{:5}{style:#} {}] {}\n\r",
                    buf.timestamp(),
                    record.level(),
                    record.target(),
                    record.args()
                )
            })
            .init(),
        false => env_logger::init(),
    }

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let code = rt.block_on(async {
        let mut core = match wayclip_daemon::linux::DaemonCore::new() {
            Ok(c) => c,
            Err(e) => {
                log::error!("{e}");
                return 1.into();
            }
        };
        match core.start().await {
            Ok(()) => 0.into(),
            Err(e) => {
                log::error!("{e}");
                1.into()
            }
        }
    });
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
    code
}
