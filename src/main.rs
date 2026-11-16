use mimalloc::MiMalloc;
use std::io::IsTerminal;
use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;
use wayclip_daemon::linux::memory::MemoryManager;
use wayclip_daemon::linux::session::env::SessionVariable;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() -> std::process::ExitCode {
    match std::io::stderr().is_terminal() {
        true => env_logger::Builder::from_default_env()
            .filter_module("discord_rich_presence", log::LevelFilter::Off)
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
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            log::error!("could not create tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let code = rt.block_on(async {
        SessionVariable::wait_for_session(Duration::from_secs(15)).await;
        let mut core = match wayclip_daemon::linux::DaemonCore::new() {
            Ok(c) => c,
            Err(e) => {
                log::error!("{e}");
                return ExitCode::FAILURE;
            }
        };
        match core.start().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                log::error!("{e}");
                ExitCode::FAILURE
            }
        }
    });
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
    code
}
