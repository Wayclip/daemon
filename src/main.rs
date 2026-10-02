#[tokio::main]
async fn main() -> std::process::ExitCode {
    unsafe {
        std::env::set_var("GLOBAL_HOTKEY_APP_ID", "com.wayclip.cli");
        std::env::set_var("GST_GL_PLATFORM", "egl");
        std::env::set_var("GST_GL_WINDOW", "surfaceless");
    }
    env_logger::init();
    let mut core = match wayclip_daemon::linux::core::DaemonCore::new() {
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
}
