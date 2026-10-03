use std::io::IsTerminal;
use std::io::Write;

fn main() -> std::process::ExitCode {
    unsafe {
        std::env::set_var("GLOBAL_HOTKEY_APP_ID", "com.wayclip.daemon");
        std::env::set_var("GST_GL_PLATFORM", "egl");
        std::env::set_var("GST_GL_WINDOW", "surfaceless");
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
