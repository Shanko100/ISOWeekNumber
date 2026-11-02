//! Run at Startup on macOS: the app registers itself as a login item with
//! SMAppService (macOS 13 and later), the system's own mechanism. It shows
//! in System Settings > General > Login Items, where the user can switch it
//! off; no launch agent file, no helper, no privileges.
//!
//! * Refused while the app runs from where it should not stay: the disk
//!   image it came on, App Translocation (an app opened straight from
//!   Downloads), a temporary or shared folder, or removable and network
//!   volumes. Applications or ~/Applications are the places to keep it.
//! * The system ties the item to this app bundle, so it follows the app
//!   when it moves and never starts another copy.

use crate::os;
use std::path::{Path, PathBuf};

/// The running app bundle (…/WeekNumber.app), if the program is in one.
fn bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(exe).ok()?;
    // …/WeekNumber.app/Contents/MacOS/WeekNumber
    let app = exe.parent()?.parent()?.parent()?;
    (app.extension()? == "app").then(|| app.to_path_buf())
}

/// Why the app may not start at login from `app`, or None.
pub fn location_problem(app: Option<&Path>) -> Option<String> {
    let hint = " Drag WeekNumber into the Applications folder, open it from there, then turn Run at Startup on again.";
    let Some(app) = app else {
        return Some(format!("WeekNumber is not running as an app bundle.{hint}"));
    };
    let s = app.to_string_lossy();
    if s.contains("/AppTranslocation/") {
        return Some(format!(
            "macOS is running WeekNumber from a temporary, read-only location.{hint}"
        ));
    }
    if s.starts_with("/Volumes/") {
        return Some(format!(
            "WeekNumber can't start at login from a disk image or an external drive.{hint}"
        ));
    }
    let risky: Vec<(PathBuf, &str)> = {
        let mut v = vec![
            (PathBuf::from("/private/tmp"), "a temporary folder"),
            (PathBuf::from("/tmp"), "a temporary folder"),
            (PathBuf::from("/private/var/folders"), "a temporary folder"),
            (PathBuf::from("/Users/Shared"), "the shared folder"),
        ];
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            v.push((home.join("Downloads"), "your Downloads folder"));
            v.push((home.join("Desktop"), "your Desktop"));
            v.push((home.join("Public"), "the Public folder"));
            v.push((home.join("Library/Mobile Documents"), "iCloud Drive"));
            v.push((home.join("Library/CloudStorage"), "a cloud folder"));
        }
        v
    };
    for (root, label) in risky {
        if app.starts_with(&root) {
            return Some(format!(
                "WeekNumber can't start at login from {label}.{hint}"
            ));
        }
    }
    None
}

pub fn is_enabled() -> bool {
    os::login_item_status() == Some(true)
}

/// Turn startup on or off; Err carries the message to show.
pub fn set(enable: bool) -> Result<(), String> {
    if enable && let Some(problem) = location_problem(bundle_path().as_deref()) {
        return Err(problem);
    }
    os::login_item_set(enable).map_err(|e| {
        if e.is_empty() {
            "macOS could not update the login item.".to_string()
        } else {
            format!("macOS could not update the login item: {e}")
        }
    })
}

/// First run: start with autostart off.
pub fn disable_on_first_run() {
    if os::login_item_status() == Some(true) {
        let _ = os::login_item_set(false);
    }
}

/// The system follows the bundle; nothing to repair at launch.
pub fn refresh_at_launch() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations() {
        assert!(location_problem(None).is_some());
        let t = Path::new("/private/var/folders/xy/T/AppTranslocation/ABC/d/WeekNumber.app");
        assert!(location_problem(Some(t)).is_some_and(|m| m.contains("temporary")));
        assert!(location_problem(Some(Path::new("/Volumes/WeekNumber/WeekNumber.app"))).is_some());
        assert!(location_problem(Some(Path::new("/Applications/WeekNumber.app"))).is_none());
        if let Some(home) = std::env::var_os("HOME") {
            let d = PathBuf::from(home).join("Downloads/WeekNumber.app");
            assert!(location_problem(Some(&d)).is_some_and(|m| m.contains("Downloads")));
        }
    }
}
