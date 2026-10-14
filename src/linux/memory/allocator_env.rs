use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
};

const MEMORY_PROFILE: &str = r#"[Service]
# wayclip-daemon
Environment=MALLOC_ARENA_MAX=2
Environment=MALLOC_TRIM_THRESHOLD_=131072
Environment=MALLOC_MMAP_THRESHOLD_=131072
Environment=MALLOC_TOP_PAD_=0
Environment=MIMALLOC_PURGE_DELAY=0
Environment=MIMALLOC_ARENA_PURGE_MULT=1
"#;

pub struct AllocatorEnv;

impl AllocatorEnv {
    pub fn ensure() {
        let path = match Self::path() {
            Ok(path) => path,
            Err(e) => {
                log::warn!("could not resolve systemd drop-in path: {e}");
                return;
            }
        };

        match fs::read_to_string(&path) {
            Ok(contents) if contents == MEMORY_PROFILE => return,
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                log::warn!("could not read {}: {e}", path.display());
                return;
            }
        }

        if let Err(e) = Self::write_atomically(&path, MEMORY_PROFILE) {
            log::warn!("could not write {}: {e}", path.display());
            return;
        }

        log::info!("wrote allocator settings to {}", path.display());

        if let Err(e) = Self::daemon_reload() {
            log::warn!(
                "wrote {} but `systemctl --user daemon-reload` failed: {e}",
                path.display()
            );
            return;
        }

        log::info!("allocator settings take effect on the next daemon start");
    }

    fn path() -> Result<PathBuf, String> {
        dirs::config_dir()
            .map(|c| c.join("systemd/user/wayclip-daemon.service.d/memory.conf"))
            .ok_or_else(|| "could not resolve config directory".to_string())
    }

    fn daemon_reload() -> Result<(), String> {
        let status = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status()
            .map_err(|e| e.to_string())?;

        if status.success() {
            Ok(())
        } else {
            Err(format!("exited with {status}"))
        }
    }

    fn write_atomically(path: &Path, contents: &str) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let temporary = path.with_extension("conf.tmp");
        fs::write(&temporary, contents).map_err(|e| e.to_string())?;
        fs::rename(&temporary, path).map_err(|e| e.to_string())
    }
}
