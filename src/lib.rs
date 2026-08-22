pub(crate) mod common;
pub use common::remux::RemuxHandler;

#[cfg(target_os = "linux")]
pub(crate) mod linux;

#[cfg(target_os = "windows")]
pub(crate) mod windows;

#[cfg(target_os = "linux")]
pub use linux::core::types::*;
#[cfg(target_os = "linux")]
pub use linux::doctor::*;
#[cfg(target_os = "linux")]
pub use linux::manager::*;
#[cfg(target_os = "linux")]
pub use linux::pipewire::*;
