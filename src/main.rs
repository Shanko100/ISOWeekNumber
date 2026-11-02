//! WeekNumber Workplace Edition: the ISO week number in the Windows
//! notification area, the macOS menu bar or the Linux system tray. No GUI
//! framework, no network code: each platform's own system interfaces.
#![deny(unsafe_code)]
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod app;
mod dates;
mod gfx;
mod holidays;
#[cfg(target_os = "linux")]
mod install;
mod json;
mod legal;
mod os;
mod pack;
mod pil;
mod png;
mod settings;
mod sha256;
mod startup;
mod theme;
mod tray;
mod tz;
mod ui;

fn main() {
    // First: DLL search path, heap and image-load hardening, DPI awareness
    // (Windows; nothing to do elsewhere).
    os::harden_process();
    // Linux: install, uninstall and the AppImage runtime options.
    #[cfg(target_os = "linux")]
    if let Some(code) = install::command_line() {
        std::process::exit(code);
    }
    os::init_com();
    std::process::exit(app::run());
}
