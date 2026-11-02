//! The operating-system layer. Every window, timer, font, tray icon and
//! clock the app uses goes through this module, which has one backend per
//! platform with the same safe interface:
//!
//! * Windows: Win32 and GDI (`win32`).
//! * macOS: AppKit, Core Text and Core Graphics (`macos`).
//! * Linux: the X11 protocol, D-Bus and TrueType fonts, spoken directly
//!   over the system's sockets and files (`linux`).
//!
//! The windows of the app are written against the Win32 message model (a
//! handler per window receiving WM_* messages); the macOS and Linux backends
//! deliver the same messages, so the drawing and behaviour are shared.

#[cfg(windows)]
mod win32;
#[cfg(windows)]
pub use ::windows::Win32::Foundation::HWND;
#[cfg(windows)]
pub use ::windows::Win32::UI::WindowsAndMessaging as wm;
#[cfg(windows)]
pub use win32::*;

#[cfg(unix)]
pub mod unix;
#[cfg(not(windows))]
pub use unix::{HWND, wm};

#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux as backend;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos as backend;
#[cfg(target_os = "macos")]
pub use macos::*;
