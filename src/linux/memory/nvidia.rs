use std::{
    ffi::OsString,
    fs,
    io::ErrorKind,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};

use wayclip_core::models::error::WayclipError;

const REEXEC_FLAG: &str = "--nvidia";
const WAYCLIP_PROFILE_JSON: &str = r#"{
  "profiles": [
    {
      "name": "Wayclip",
      "settings": [
        { "key": "GLVidHeapReuseRatio", "value": 0 }
      ]
    }
  ],
  "rules": [
    {
      "pattern": { "feature": "procname", "matches": "wayclip-daemon" },
      "profile": "Wayclip"
    }
  ]
}
"#;

pub struct Nvidia;

impl Nvidia {
    pub fn ensure_vram_profile() -> Result<bool, WayclipError> {
        let path = Self::profile_path()?;

        match fs::read_to_string(&path) {
            // If exists & matches -> we didnt cahnge
            Ok(contents) if contents == WAYCLIP_PROFILE_JSON => return Ok(false),
            // If exists & doesnt match -> create
            Ok(_) => {}
            // If doesnt exist -> create
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            // If error -> error
            Err(e) => return Err(e.into()),
        }

        Self::write_atomically(&path, WAYCLIP_PROFILE_JSON)?;

        log::info!("Wrote NVIDIA video memory profile to {}", path.display());
        Ok(true)
    }

    pub fn reexec(changed: bool) -> Result<(), WayclipError> {
        if !changed {
            return Ok(());
        }

        if std::env::args().any(|arg| arg == REEXEC_FLAG) {
            log::warn!(
                "NVIDIA video memory profile still not applied after re-exec; check {} is writable and valid JSON",
                Nvidia::profile_path()?.display()
            );
            return Ok(());
        }

        let exe = std::env::current_exe().map_err(|e| {
            WayclipError::Video(format!("Could not resolve own executable path: {e}").into())
        })?;

        log::info!("Re-executing so the NVIDIA video memory profile applies to this process");

        let args = std::env::args_os()
            .skip(1)
            .chain([OsString::from(REEXEC_FLAG)]);
        let error = Command::new(exe).args(args).exec();

        Err(WayclipError::Video(
            format!("Could not re-exec for NVIDIA video memory profile: {error}").into(),
        ))
    }

    fn profile_path() -> Result<PathBuf, WayclipError> {
        let home = dirs::home_dir()
            .ok_or_else(|| WayclipError::NotFound("No home dir for NVIDIA profile".into()))?;
        Ok(home
            .join(".nv")
            .join("nvidia-application-profiles-rc.d")
            .join("50-wayclip.json"))
    }

    fn write_atomically(path: &Path, contents: &str) -> Result<(), WayclipError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, contents)?;
        fs::rename(&temporary, path)?;
        Ok(())
    }
}
