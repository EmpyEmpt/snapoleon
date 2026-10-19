//! OS text recognition: Windows.Media.Ocr, Apple Vision.

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows::{languages, recognize, status};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::{languages, recognize, status};

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod linux;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) use linux::{languages, recognize, status};
