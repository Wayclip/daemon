pub(crate) mod common;
pub use common::video::preview::*;

#[cfg(feature = "linux")]
pub(crate) mod linux;

#[cfg(feature = "windows")]
pub(crate) mod windows;
