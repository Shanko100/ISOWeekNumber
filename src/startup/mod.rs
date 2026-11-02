//! Run at Startup, per platform: the HKCU Run value on Windows, a login
//! item (SMAppService) on macOS, an XDG autostart entry on Linux. Each
//! refuses to start the app from places where someone else could swap the
//! program (downloads, temporary, shared, removable or network locations)
//! and never repoints an entry to a different copy.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod win32;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(windows)]
pub use win32::*;
