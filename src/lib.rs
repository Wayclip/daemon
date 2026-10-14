pub mod common;

#[cfg(all(feature = "linux", feature = "windows"))]
compile_error!("Windows & Linux mutually exclusive");

#[cfg(feature = "linux")]
pub mod linux;

#[cfg(feature = "windows")]
pub(crate) mod windows;
